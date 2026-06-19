# A BigInt slice step clamps to i64::MIN/MAX; combined with a non-zero start
# the per-step advance must saturate (not wrap), so a forward/backward slice
# yields exactly the first/last element. Regression for the generic
# slice_target_indices path shared by list / str / tuple / bytes (#2671).
BIG = 1 << 70
NEG = -(1 << 70)

for s in ([1, 2, 3, 4, 5, 6], "abcdef", (1, 2, 3, 4, 5, 6), b'abcdef'):
    print(s[2::BIG])      # third element only
    print(s[3::BIG])      # fourth element only
    print(s[3::-BIG])     # fourth element only (backward)
    print(s[-1::-BIG])    # last element only
    print(s[5::BIG])      # last element only
    print(s[BIG::BIG])    # empty (start past end)
    print(s[NEG::BIG])    # first element only
    print(s[NEG:BIG:BIG]) # first element only

# Extended-slice assignment with non-zero start + BigInt step (one target).
L = [1, 2, 3, 4, 5, 6]
L[2::BIG] = [99]
print(L)

# Extended-slice deletion with non-zero start + BigInt step.
L2 = [1, 2, 3, 4, 5, 6]
del L2[2::BIG]
print(L2)

# i64 boundary literal steps must not panic.
print([1, 2, 3][1::9223372036854775807])
print([1, 2, 3][1::-9223372036854775808])

# Normal small steps unaffected.
print([1, 2, 3, 4, 5, 6][::2])
print("abcdef"[1:5:2])
