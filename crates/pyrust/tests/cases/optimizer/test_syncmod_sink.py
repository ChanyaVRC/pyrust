# Parity fixture for pass_syncmod_sink: SyncModuleGlobal sinking from tight loops.
# After optimization, module-level while loops with no function calls should
# produce correct results even though SyncModuleGlobal is deferred to loop exit.

i = 0
n = 100
s = 0
while i < n:
    s += i
    i += 1
print(s)  # 4950

acc = 0
for _ in range(50):
    acc += 1
print(acc)  # 50
