"""Companion imported module with dunder-mask-colliding names (issue #2925).

`beta0` and `delta` both collide with the 2-bit Bloom `env_binding_interest`
mask formed by the standard pre-seeded module dunders, while `alpha`, `gamma`,
and `epsilon` do not.
"""

alpha = 1
beta0 = 2
gamma = 3
delta = 4
epsilon = 5

exec("exec_alpha = 10\nexec_beta = 20\nfirst3 = 30\nexec_gamma = 40\n")
