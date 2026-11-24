# CPython 3.12 parity tests for `difflib` module (issue #3147).
import difflib
from difflib import (
    Differ,
    HtmlDiff,
    IS_CHARACTER_JUNK,
    IS_LINE_JUNK,
    Match,
    SequenceMatcher,
    context_diff,
    diff_bytes,
    get_close_matches,
    ndiff,
    restore,
    unified_diff,
)

# Module surface
print("all:", difflib.__all__)
print("Match fields:", Match._fields, Match.__module__)
m = Match(1, 2, 3)
print("Match:", m, m.a, m.b, m.size, tuple(m))
print("GenericAlias:", SequenceMatcher[str])

# SequenceMatcher basics
s = SequenceMatcher(None, "abcd", "bcde")
print("ratio:", s.ratio(), s.quick_ratio(), s.real_quick_ratio())
print("blocks:", s.get_matching_blocks())
print("opcodes:", s.get_opcodes())

# set_seq1 / set_seq2 / set_seqs
s.set_seq1("bcde")
print("after set_seq1:", s.ratio(), s.get_opcodes())
s.set_seq2("abcd")
print("after set_seq2:", s.ratio())
s.set_seqs("qabxcd", "abycdf")
print("opcodes qabxcd->abycdf:", s.get_opcodes())

# Empty and identical sequences
print("empty:", SequenceMatcher(None, "", "").ratio(), SequenceMatcher(None, "", "").get_opcodes())
print("one empty:", SequenceMatcher(None, "abc", "").ratio(), SequenceMatcher(None, "", "abc").get_opcodes())
print("disjoint:", SequenceMatcher(None, "abc", "xyz").ratio(), SequenceMatcher(None, "abc", "xyz").get_opcodes())

# find_longest_match with and without isjunk and subranges
sm1 = SequenceMatcher(None, " abcd", "abcd abcd")
print("flm no junk:", sm1.find_longest_match(0, 5, 0, 9), sm1.find_longest_match())
sm2 = SequenceMatcher(lambda x: x == " ", " abcd", "abcd abcd")
print("flm space junk:", sm2.find_longest_match(0, 5, 0, 9))
print("flm no match:", SequenceMatcher(None, "ab", "c").find_longest_match(0, 2, 0, 1))

# Non-string sequences (lists of ints)
sm_list = SequenceMatcher(None, [1, 2, 3, 4, 5], [1, 2, 9, 4, 5, 6])
print("list opcodes:", sm_list.get_opcodes())
print("list ratio:", round(sm_list.ratio(), 6))

# autojunk=True vs autojunk=False (>= 200 elements triggers heuristic)
long_a = "a" * 205 + "b"
long_b = "a" * 205 + "c"
sm_auto_on = SequenceMatcher(None, long_a, long_b, autojunk=True)
sm_auto_off = SequenceMatcher(None, long_a, long_b, autojunk=False)
print("autojunk on bpopular:", sorted(sm_auto_on.bpopular), "ratio:", round(sm_auto_on.ratio(), 4))
print("autojunk off bpopular:", sorted(sm_auto_off.bpopular), "ratio:", round(sm_auto_off.ratio(), 4))

# get_grouped_opcodes
ga = list(map(str, range(1, 40)))
gb = ga[:]
gb[8:8] = ["i"]
gb[20] += "x"
gb[23:28] = []
gb[30] += "y"
print("grouped:", list(SequenceMatcher(None, ga, gb).get_grouped_opcodes()))
print("grouped n=1:", list(SequenceMatcher(None, ga, gb).get_grouped_opcodes(1)))
print("grouped identical:", list(SequenceMatcher(None, ["a", "b"], ["a", "b"]).get_grouped_opcodes()))

# IS_LINE_JUNK and IS_CHARACTER_JUNK
for line in ["\n", "   \n", "  #   \n", "#\n", "##\n", "# comment\n", "hello\n", ""]:
    print("line_junk", repr(line), IS_LINE_JUNK(line))
print("line_junk custom pat:", IS_LINE_JUNK("foo", lambda s: s if s == "foo" else None))
for ch in [" ", "\t", "\n", "a", ""]:
    print("char_junk", repr(ch), IS_CHARACTER_JUNK(ch))
print("char_junk custom ws:", IS_CHARACTER_JUNK("x", "xy"), IS_CHARACTER_JUNK("z", "xy"))

# Differ and ndiff + restore
text1 = [
    "  1. Beautiful is better than ugly.\n",
    "  2. Explicit is better than implicit.\n",
    "  3. Simple is better than complex.\n",
    "  4. Complex is better than complicated.\n",
]
text2 = [
    "  1. Beautiful is better than ugly.\n",
    "  3.   Simple is better than complex.\n",
    "  4. Complicated is better than complex.\n",
    "  5. Flat is better than nested.\n",
]
diff_out = list(Differ().compare(text1, text2))
print("Differ compare:")
for line in diff_out:
    print(repr(line))

ndiff_out = list(ndiff("one\ntwo\nthree\n".splitlines(True), "ore\ntree\nemu\n".splitlines(True)))
print("ndiff:")
for line in ndiff_out:
    print(repr(line))

print("restore 1:", list(restore(ndiff_out, 1)))
print("restore 2:", list(restore(ndiff_out, 2)))
try:
    list(restore(ndiff_out, 3))
except ValueError as e:
    print("restore bad which:", str(e))

# unified_diff
ua = "one two three four".split()
ub = "zero one tree four".split()
print("unified_diff:")
for line in unified_diff(
    ua, ub, "Original", "Current", "2005-01-26 23:30:50", "2010-04-02 10:20:52", lineterm=""
):
    print(repr(line))
print("unified identical:", list(unified_diff(["a\n"], ["a\n"])))
print("unified n=0:", list(unified_diff(["a\n", "b\n", "c\n"], ["a\n", "x\n", "c\n"], n=0)))

# context_diff
ca = "one\ntwo\nthree\nfour\n".splitlines(True)
cb = "zero\none\ntree\nfour\n".splitlines(True)
print("context_diff:")
for line in context_diff(ca, cb, "Original", "Current"):
    print(repr(line))
print("context identical:", list(context_diff(["a\n"], ["a\n"])))

# Type checks on unified_diff / context_diff
for bad_call in [
    lambda: list(unified_diff([b"a\n"], [b"b\n"])),
    lambda: list(context_diff(["a\n"], [b"b\n"])),
    lambda: list(context_diff(["a\n"], ["b\n"], fromfile=b"old")),
]:
    try:
        bad_call()
        print("expected TypeError")
    except TypeError:
        print("diff TypeError ok")

# diff_bytes (including non-ASCII raw bytes round-tripped via surrogateescape)
ba = [b"alpha\n", b"beta \xff\n", b"gamma\n"]
bb = [b"alpha\n", b"BETA \xfe\n", b"gamma\n"]
print("diff_bytes unified:", list(diff_bytes(unified_diff, ba, bb, b"a.bin", b"b.bin")))
print("diff_bytes context:", list(diff_bytes(context_diff, ba, bb, b"a.bin", b"b.bin")))
try:
    list(diff_bytes(unified_diff, ["str"], [b"bytes"]))
except TypeError:
    print("diff_bytes TypeError ok")

# get_close_matches
words = ["ape", "apple", "peach", "puppy"]
print("close default:", get_close_matches("appel", words))
print("close n=1:", get_close_matches("appel", words, n=1))
print("close cutoff=0.8:", get_close_matches("appel", words, cutoff=0.8))
print("close none:", get_close_matches("zzzzz", words))

for bad_n in (0, -1):
    try:
        get_close_matches("a", ["a"], n=bad_n)
    except ValueError as e:
        print("bad n:", str(e))

for bad_c in (-0.1, 1.1):
    try:
        get_close_matches("a", ["a"], cutoff=bad_c)
    except ValueError as e:
        print("bad cutoff:", str(e))

# HtmlDiff basic smoke test
table = HtmlDiff().make_table(["hello\n", "world\n"], ["hello\n", "python\n"], "a", "b")
print("HtmlDiff has table:", "<table" in table and "diff_chg" in table and "python" in table)

print("difflib ok")
