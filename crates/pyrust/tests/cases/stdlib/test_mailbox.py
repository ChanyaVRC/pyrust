"""CPython 3.12 parity tests for the `mailbox` module (issue #3257)."""

import contextlib
import io
import os
import tempfile
import email.message
import mailbox

# 1. Exception hierarchy & __all__
print("mailbox_all", mailbox.__all__)
assert issubclass(mailbox.Error, Exception)
assert issubclass(mailbox.NoSuchMailboxError, mailbox.Error)
assert issubclass(mailbox.NotEmptyError, mailbox.Error)
assert issubclass(mailbox.ExternalClashError, mailbox.Error)
assert issubclass(mailbox.FormatError, mailbox.Error)

# 2. Message classes & cross-format conversion
raw_msg = "From: alice@example.com\nTo: bob@example.com\nSubject: Hello\n\nBody text\n"
base_m = mailbox.Message(raw_msg)
assert isinstance(base_m, email.message.Message)
assert base_m["Subject"] == "Hello"
assert base_m.get_payload() == "Body text\n"

md_m = mailbox.MaildirMessage(raw_msg)
assert md_m.get_subdir() == "new"
assert md_m.get_flags() == ""
md_m.set_subdir("cur")
assert md_m.get_subdir() == "cur"
md_m.set_flags("RS")
assert md_m.get_flags() == "RS"
assert md_m.get_info() == "2,RS"
md_m.add_flag("F")
assert md_m.get_flags() == "FRS"
md_m.remove_flag("R")
assert md_m.get_flags() == "FS"
md_m.set_date(1700000000.5)
assert md_m.get_date() == 1700000000.5

try:
    md_m.set_subdir("bad")
    assert False
except ValueError:
    print("md_bad_subdir_ok", True)

try:
    md_m.set_info(123)
    assert False
except TypeError:
    print("md_bad_info_ok", True)

# Cross-conversion: MaildirMessage -> mboxMessage -> MHMessage -> BabylMessage -> MaildirMessage
mbox_m = mailbox.mboxMessage(md_m)
assert mbox_m["Subject"] == "Hello"
assert "R" in mbox_m.get_flags() and "O" in mbox_m.get_flags() and "F" in mbox_m.get_flags()
mbox_m.set_from("alice@example.com")
assert mbox_m.get_from() == "alice@example.com"
mbox_m.add_flag("A")
assert "A" in mbox_m.get_flags()
mbox_m.remove_flag("F")
assert "F" not in mbox_m.get_flags()

mh_m = mailbox.MHMessage(mbox_m)
assert "replied" in mh_m.get_sequences()
assert "unseen" not in mh_m.get_sequences()
mh_m.add_sequence("custom")
assert "custom" in mh_m.get_sequences()
mh_m.remove_sequence("custom")
assert "custom" not in mh_m.get_sequences()

bab_m = mailbox.BabylMessage(mh_m)
assert "answered" in bab_m.get_labels()
bab_m.add_label("work")
assert "work" in bab_m.get_labels()
bab_m.update_visible()
assert bab_m.get_visible()["Subject"] == "Hello"
bab_m.remove_label("work")
assert "work" not in bab_m.get_labels()

md_m2 = mailbox.MaildirMessage(bab_m)
assert md_m2.get_subdir() == "cur"
assert "S" in md_m2.get_flags() and "R" in md_m2.get_flags()
print("message_conversions_ok", True)

with tempfile.TemporaryDirectory() as tmpdir:
    # 3. Maildir CRUD, flags, folders, custom factory
    missing_md = os.path.join(tmpdir, "does_not_exist_md")
    try:
        mailbox.Maildir(missing_md, create=False)
        assert False
    except mailbox.NoSuchMailboxError:
        print("maildir_nosuch_ok", True)

    md_path = os.path.join(tmpdir, "my_maildir")
    md = mailbox.Maildir(md_path)
    # Use '!' as info separator so Maildir flags work identically on Windows and POSIX
    md.colon = "!"
    k1 = md.add(md_m)
    k2 = md.add("From: bob@example.com\nSubject: Second\n\nHello 2\n")
    assert len(md) == 2
    assert k1 in md and k2 in md and "missing" not in md
    assert set(md.keys()) == {k1, k2}
    assert len(md.values()) == 2
    assert len(md.items()) == 2

    got1 = md[k1]
    assert got1["Subject"] == "Hello"
    assert got1.get_subdir() == "cur"
    assert got1.get_flags() == "FS"
    assert b"Body text" in md.get_bytes(k1)
    assert "Subject: Hello" in md.get_string(k1)
    with md.get_file(k1) as f:
        data = f.read()
        assert b"Body text" in data
        f.seek(0)
        assert f.tell() == 0
        first_line = f.readline()
        assert first_line.startswith(b"From: ")

    md[k2] = "From: bob@example.com\nSubject: Updated\n\nNew body\n"
    assert md[k2]["Subject"] == "Updated"

    # Folders
    sub = md.add_folder("Archive")
    assert "Archive" in md.list_folders()
    sk = sub.add("Subject: Archived\n\nOld mail\n")
    try:
        md.remove_folder("Archive")
        assert False
    except mailbox.NotEmptyError:
        print("maildir_notempty_ok", True)
    sub.remove(sk)
    md.remove_folder("Archive")
    assert "Archive" not in md.list_folders()

    popped = md.pop(k2)
    assert popped["Subject"] == "Updated"
    assert len(md) == 1
    assert md.get("missing", "fallback") == "fallback"
    md.discard("missing")
    md.clean()
    md.clear()
    assert len(md) == 0
    md.close()
    print("maildir_ok", True)

    # 4. mbox CRUD, From_ mangling, flush/reopen, lock/unlock
    missing_mbox = os.path.join(tmpdir, "does_not_exist.mbox")
    try:
        mailbox.mbox(missing_mbox, create=False)
        assert False
    except mailbox.NoSuchMailboxError:
        print("mbox_nosuch_ok", True)

    mbox_path = os.path.join(tmpdir, "test.mbox")
    mb = mailbox.mbox(mbox_path)
    mb.lock()
    try:
        mb_other = mailbox.mbox(mbox_path)
        try:
            mb_other.lock()
            assert False
        except mailbox.ExternalClashError:
            print("mbox_lock_clash_ok", True)
        finally:
            mb_other.close()

        m_a = mailbox.mboxMessage("From: a@example.com\nSubject: One\n\nHello\nFrom body line\n")
        m_a.set_from("a@example.com")
        m_a.set_flags("RO")
        mk0 = mb.add(m_a)
        mk1 = mb.add("From: b@example.com\nSubject: Two\n\nSecond body\n")
        assert (mk0, mk1) == (0, 1)
        assert len(mb) == 2
        mb.flush()

        assert mb[0]["Subject"] == "One"
        assert ">From body line" in mb.get_bytes(0).decode("ascii")
        assert mb.get_bytes(0, from_=True).startswith(b"From a@example.com")
        with contextlib.closing(mb.get_file(1)) as mf:
            assert b"Second body" in mf.read()

        mb[1] = "From: b@example.com\nSubject: Two Edited\n\nEdited body\n"
        mb.remove(0)
        mb.flush()
    finally:
        mb.unlock()
        mb.close()

    # Reopen mbox and check persistence
    mb2 = mailbox.mbox(mbox_path, create=False)
    assert len(mb2) == 1
    assert mb2[0]["Subject"] == "Two Edited"
    assert mb2[0].get_payload() == "Edited body\n"
    mb2.close()
    print("mbox_ok", True)

    # 5. MMDF CRUD & persistence
    mmdf_path = os.path.join(tmpdir, "test.mmdf")
    mm = mailbox.MMDF(mmdf_path)
    mm.add("From: u1@example.com\nSubject: MMDF 1\n\nBody 1\n")
    mm.add("From: u2@example.com\nSubject: MMDF 2\n\nBody 2\n")
    mm.flush()
    mm.remove(0)
    mm.close()

    mm2 = mailbox.MMDF(mmdf_path, create=False)
    assert len(mm2) == 1
    assert mm2[0]["Subject"] == "MMDF 2"
    mm2.close()
    print("mmdf_ok", True)

    # 6. MH CRUD, sequences, pack, folders
    mh_path = os.path.join(tmpdir, "test_mh")
    mh = mailbox.MH(mh_path)
    mhm1 = mailbox.MHMessage("Subject: MH 1\n\nOne\n")
    mhm1.add_sequence("unseen")
    mhm2 = mailbox.MHMessage("Subject: MH 2\n\nTwo\n")
    mhm2.add_sequence("flagged")
    mhm3 = mailbox.MHMessage("Subject: MH 3\n\nThree\n")
    mhm3.add_sequence("unseen")
    hk1 = mh.add(mhm1)
    hk2 = mh.add(mhm2)
    hk3 = mh.add(mhm3)
    assert (hk1, hk2, hk3) == (1, 2, 3)
    print("mh_seqs_initial", sorted(mh.get_sequences().items()))
    mh.remove(2)
    mh.pack()
    assert mh.keys() == [1, 2]
    assert mh[2]["Subject"] == "MH 3"
    print("mh_seqs_packed", sorted(mh.get_sequences().items()))

    mh.set_sequences({"batch": [1, 2]})
    assert mh.get_sequences() == {"batch": [1, 2]}

    sub_mh = mh.add_folder("sub")
    assert "sub" in mh.list_folders()
    mh.remove_folder("sub")
    assert "sub" not in mh.list_folders()

    # FormatError on corrupt .mh_sequences
    with open(os.path.join(mh_path, ".mh_sequences"), "w", encoding="ascii") as sf:
        sf.write("corrupt line without colon\n")
    try:
        mh.get_sequences()
        assert False
    except mailbox.FormatError:
        print("mh_format_error_ok", True)
    mh.close()
    print("mh_ok", True)

    # 7. Babyl CRUD, labels, visible headers, persistence
    bab_path = os.path.join(tmpdir, "test.babyl")
    bab = mailbox.Babyl(bab_path)
    bm1 = mailbox.BabylMessage("From: alice@example.com\nSubject: Babyl 1\nX-Hidden: secret\n\nHello Babyl\n\nLine 2\n")
    bm1.add_label("unseen")
    bm1.add_label("project")
    bm1.update_visible()
    assert bm1.get_visible()["Subject"] == "Babyl 1"
    bk0 = bab.add(bm1)
    bk1 = bab.add("From: bob@example.com\nSubject: Babyl 2\n\nSecond Babyl\n")
    assert (bk0, bk1) == (0, 1)
    assert bab.get_labels() == ["project"]
    bab.flush()
    bab.close()

    bab2 = mailbox.Babyl(bab_path, create=False)
    assert len(bab2) == 2
    assert bab2[0]["Subject"] == "Babyl 1"
    assert bab2[1]["Subject"] == "Babyl 2"
    assert b"Second Babyl" in bab2.get_bytes(1)
    with contextlib.closing(bab2.get_file(1)) as bf:
        assert b"Second Babyl" in bf.read()
    bab2.remove(0)
    bab2.close()

    bab3 = mailbox.Babyl(bab_path, create=False)
    assert len(bab3) == 1
    assert bab3[0]["Subject"] == "Babyl 2"
    bab3.close()
    print("babyl_ok", True)

print("ALL MAILBOX TESTS PASSED")
