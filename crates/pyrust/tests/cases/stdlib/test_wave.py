import io
import os
import pathlib
import struct
import tempfile
import wave

# 1. Module surface & exceptions
assert issubclass(wave.Error, Exception)
assert wave.WAVE_FORMAT_PCM == 1
print("wave_surface_ok")

# 2. Byte-for-byte RIFF/WAVE PCM writing & reading via io.BytesIO
buf = io.BytesIO()
frames_1 = struct.pack("<hhhh", 0, 1000, -1000, 32767)
frames_2 = struct.pack("<hhhh", -32768, 500, -500, 0)

with wave.open(buf, "wb") as w:
    w.setnchannels(2)
    w.setsampwidth(2)
    w.setframerate(44100)
    w.setcomptype("NONE", "not compressed")
    assert w.getnchannels() == 2
    assert w.getsampwidth() == 2
    assert w.getframerate() == 44100
    assert w.getcomptype() == "NONE"
    assert w.getcompname() == "not compressed"
    assert w.tell() == 0
    w.writeframesraw(frames_1)
    assert w.tell() == 2
    w.writeframes(frames_2)
    assert w.tell() == 4
    assert w.getnframes() == 4

wav_bytes = buf.getvalue()
print("wav_len:", len(wav_bytes))
print("wav_hex:", wav_bytes.hex())
assert wav_bytes[:4] == b"RIFF"
assert wav_bytes[8:12] == b"WAVE"
assert len(wav_bytes) == 44 + len(frames_1) + len(frames_2)

buf.seek(0)
with wave.open(buf, "rb") as r:
    assert r.getnchannels() == 2
    assert r.getsampwidth() == 2
    assert r.getframerate() == 44100
    assert r.getnframes() == 4
    assert r.getcomptype() == "NONE"
    assert r.getcompname() == "not compressed"
    params = r.getparams()
    assert isinstance(params, tuple)
    assert tuple(params) == (2, 2, 44100, 4, "NONE", "not compressed")
    assert params.nchannels == 2
    assert params.sampwidth == 2
    assert params.framerate == 44100
    assert params.nframes == 4
    assert params.comptype == "NONE"
    assert params.compname == "not compressed"
    nc, sw, fr, nf, ct, cn = params
    assert (nc, sw, fr, nf, ct, cn) == (2, 2, 44100, 4, "NONE", "not compressed")

    assert r.tell() == 0
    assert r.readframes(2) == frames_1
    assert r.tell() == 2
    assert r.readframes(0) == b""
    assert r.tell() == 2
    assert r.readframes(10) == frames_2
    assert r.tell() == 4
    assert r.readframes(1) == b""

    r.rewind()
    assert r.tell() == 0
    assert r.readframes(1) == frames_1[:4]

    r.setpos(3)
    assert r.tell() == 3
    assert r.readframes(1) == frames_2[4:]
print("wave_bytesio_ok")

# 3. setparams + PathLike + filename on disk + 1-byte / 3-byte / 4-byte widths
with tempfile.TemporaryDirectory() as tmpdir:
    p_str = os.path.join(tmpdir, "mono8.wav")
    with wave.open(p_str, "wb") as w:
        w.setparams((1, 1, 8000, 5, "NONE", "not compressed"))
        w.writeframes(b"\x80\x81\x82\x83\x84")

    with wave.open(p_str, "rb") as r:
        assert tuple(r.getparams()) == (1, 1, 8000, 5, "NONE", "not compressed")
        assert r.readframes(5) == b"\x80\x81\x82\x83\x84"

    p_path = pathlib.Path(tmpdir) / "stereo24.wav"
    raw24 = b"\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c"
    with wave.open(str(p_path), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(3)
        w.setframerate(48000.4)
        w.writeframes(raw24)

    with wave.open(str(p_path), "rb") as r:
        assert r.getnchannels() == 2
        assert r.getsampwidth() == 3
        assert r.getframerate() == 48000
        assert r.getnframes() == 2
        assert r.readframes(2) == raw24

    # Mode inferred from open file object
    with open(p_str, "rb") as raw_f:
        with wave.open(raw_f) as r:
            assert r.getnframes() == 5
            assert r.readframes(5) == b"\x80\x81\x82\x83\x84"
print("wave_disk_ok")

# 4. Reading WAV with extra odd-length JUNK chunk before fmt & data
fmt_chunk = b"fmt " + struct.pack("<LHHLLHH", 16, 1, 1, 16000, 32000, 2, 16)
junk_chunk = b"JUNK" + struct.pack("<L", 3) + b"abc\x00"  # odd size 3 + 1 pad byte
pcm_data = b"\x10\x00\x20\x00"
data_chunk = b"data" + struct.pack("<L", len(pcm_data)) + pcm_data
body = b"WAVE" + junk_chunk + fmt_chunk + data_chunk
custom_wav = b"RIFF" + struct.pack("<L", len(body)) + body
with wave.open(io.BytesIO(custom_wav), "rb") as r:
    assert r.getnchannels() == 1
    assert r.getsampwidth() == 2
    assert r.getframerate() == 16000
    assert r.getnframes() == 2
    assert r.readframes(2) == pcm_data
print("wave_extra_chunks_ok")

# 5. Error handling
for bad_blob in (
    b"NOPE" + b"\x00" * 40,
    b"RIFF" + struct.pack("<L", 4) + b"NOPE",
    b"RIFF" + struct.pack("<L", 12) + b"WAVE" + b"data" + struct.pack("<L", 0),
    b"RIFF" + struct.pack("<L", 28) + b"WAVE" + fmt_chunk,
):
    try:
        wave.open(io.BytesIO(bad_blob), "rb")
        assert False, "expected wave.Error"
    except wave.Error:
        pass

try:
    wave.open(io.BytesIO(), "x")
    assert False
except wave.Error:
    pass

with wave.open(io.BytesIO(wav_bytes), "rb") as r:
    for bad_pos in (-1, 5):
        try:
            r.setpos(bad_pos)
            assert False
        except wave.Error:
            pass

out = io.BytesIO()
with wave.open(out, "wb") as w:
    for fn in (
        lambda: w.setnchannels(0),
        lambda: w.setsampwidth(0),
        lambda: w.setsampwidth(5),
        lambda: w.setframerate(0),
        lambda: w.setcomptype("ULAW", "u-law"),
    ):
        try:
            fn()
            assert False
        except wave.Error:
            pass
    w.setnchannels(1)
    w.setsampwidth(2)
    w.setframerate(8000)
    w.writeframes(b"\x00\x00")
    try:
        w.setnchannels(2)
        assert False
    except wave.Error:
        pass
print("wave_errors_ok")
