# Issue #2925: `env_binding_interest` Bloom false positives on fast-local
# module writes must not insert absent keys into `EnvValues` or perturb
# `globals()` / `locals()` / `vars(mod)` insertion order.
#
# Against the seeded module dunders (`__name__`, `__doc__`, `__package__`,
# `__loader__`, `__spec__`, `__annotations__`, `__builtins__`, `__file__`,
# `__cached__`), the identifiers `beta0`, `delta`, `first3`, and `v_4` collide
# with `env_binding_interest`, while `alpha`, `gamma`, and `epsilon` do not.

# 1. `exec()` in the current module namespace before `globals()` is called:
#    the colliding names (`beta0`, `delta`) must keep their binding positions
#    after `alpha` and `gamma` rather than being inserted early into `values`.
exec("alpha = 1\nbeta0 = 2\ngamma = 3\ndelta = 4\nepsilon = 5\n")

# 2. Top-level fast-local bindings with colliding names interleaved after
#    non-colliding ones, also before `globals()` is exposed.
top_a = 10
v_4 = 20
top_b = 30
first3 = 40
top_c = 50

# Rebinding colliding names must preserve their positions and values.
v_4 = 200
first3 = 400
exec("delta = 400\n")


# Updating a real pre-existing env binding (e.g. a pre-seeded dunder or a name
# created via `global` from a function) BEFORE `globals()` is exposed must
# still synchronize so nested functions observe the updated value.
def read_doc():
    return __doc__


__doc__ = "updated-before-globals"
doc_from_fn = read_doc()

print("globals:", [k for k in globals() if not k.startswith("__")])
print("locals == globals:", list(locals()) == list(globals()))
print("doc_from_fn:", doc_from_fn, globals()["__doc__"])
print("values:", alpha, beta0, gamma, delta, epsilon, top_a, v_4, top_b, first3, top_c)

# 3. Updating a real pre-existing env binding (e.g. a dunder or a name created
#    via `global` from a function) must STILL synchronize so nested functions
#    observe the updated value.
def seed_from_fn():
    global synced_name
    synced_name = "from-fn"


def read_synced():
    return synced_name


seed_from_fn()
synced_name = "from-module"
print("synced_name:", read_synced(), globals()["synced_name"])

# 4. Imported filesystem module with colliding names placed after non-colliding
#    names, including an `exec()` inside the module body.
import _globals_mask_collision_mod_2925 as mod

print("mod vars:", [k for k in vars(mod) if not k.startswith("__")])
print("mod dict:", [k for k in mod.__dict__ if not k.startswith("__")])
print(
    "mod values:",
    mod.alpha,
    mod.beta0,
    mod.gamma,
    mod.delta,
    mod.epsilon,
    mod.exec_alpha,
    mod.exec_beta,
    mod.first3,
    mod.exec_gamma,
)
