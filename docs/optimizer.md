# PyRust Optimizer

PyRust applies a peephole optimizer to every compiled function (and nested function) before execution. The optimizer runs over the bytecode instruction list and applies passes in a fixed order. Each pass is a pure function that takes a `Vec<Insn>` and returns a transformed `Vec<Insn>`.

## Pass pipeline

The passes are applied in the following order:

| # | Pass | What it does |
|---|------|--------------|
| 1 | **Jump threading** | Redirects any jump whose target is itself a `Jump` to the chain's final non-jump destination. Reduces the depth of nested control-flow jumps. |
| 2 | **BinOp–constant fusion** | Fuses `LoadConst(r, c) + BinOp(dst, lhs, op, r)` into `BinOpConst(dst, lhs, op, c)` when `r` is a temporary register and not live past the `BinOp`. Also handles the commutative case (`0 + x`, `1 * x`). |
| 3 | **Constant tuple folding** | Folds a sequence of `LoadConst` instructions feeding a `BuildTuple` into a single `LoadConst` pointing to a pre-built tuple constant. |
| 4 | **Constant folding** | Evaluates `BinOp`/`BinOpConst` instructions at compile time when both operands are statically known. Clears its knowledge map at branch targets and back-edges to avoid incorrect folding across control flow. |
| 5 | **Algebraic simplification** | Replaces integer-arithmetic identities with cheaper forms: `x + 0 → x`, `x * 1 → x`, `x * 0 → 0`, `x ** 1 → x`, `x ** 0 → 1`. |
| 6 | **Unary constant folding** | Fuses `LoadConst(r, c) + UnaryOp(dst, op, r)` into a single `LoadConst` for `Neg`, `Not`, and `BitNot` applied to integer or float constants. |
| 7 | **Constant branch elimination** | Converts conditional jumps whose condition register holds a known constant into unconditional `Jump`s, removing dead branches. |
| 8 | **Compare–jump fusion** | Merges comparison instructions with the immediately following conditional jump: `BinOp(r, …) + JumpIfFalse(r, k)` → `CmpJumpIfFalse(…, k)`. Eliminates the intermediate result register. |
| 9 | **`not` inversion** | Absorbs `UnaryOp(r, Not, src)` into the following conditional jump by inverting the branch sense (`Not + JumpIfFalse` → `JumpIfTrue`). |
| 10 | **`BinOpInPlace` downgrade** | Converts `BinOpInPlace(dst, lhs, op, rhs)` to `BinOp(dst, lhs, op, rhs)` when the in-place register is a dead temporary, eliminating an unnecessary deep-copy. |
| 11 | **Dead code elimination** | Removes instructions unreachable from the entry point using BFS over all possible control-flow successors (including exception handler targets). |
| 12 | **Dead store elimination** | Removes writes to temporary registers (`>= num_locals`) whose stored value is overwritten before any use. Conservatively skips instructions followed by a back-edge (loop guard). |
| 13 | **Copy propagation** | Eliminates `Move(dst, src)` instructions by substituting `src` for all reads of `dst` within the same basic block (forward dataflow). Kills aliases at jump targets and at writes. Never substitutes the receiver register in in-place mutation instructions (`ListAppend`, `DictUpdate`, etc.). |
| 14 | **Trivial nop removal** | Removes instructions with no observable effect: `Jump(0)` (self-jump) and `Move(r, r)` (self-copy). These are typically produced as residue by earlier passes. |
| 15 | **Constant pool compaction** | Removes unreferenced constants from the pool and renumbers all constant indices throughout the instruction list. |

## Guards

Several passes include guards to maintain correctness in the presence of loops:

- **Back-edge guard** (passes 2, 12): A pass checks `slice_has_back_edge` on the instructions following a candidate. If a backward jump (`Jump(k < 0)`, `ForIter`, etc.) is reachable, the transformation is skipped to avoid miscompiling loop-carried values.
- **Basic-block boundary** (pass 13): Copy propagation clears its alias map at every instruction that is a jump target (reachable from more than one predecessor), since the contents of a register at such a point depend on which path was taken.

## Recursive application

`optimize_fn_code` recurses into every `UserFunction` value in the constant pool so that nested function definitions (closures, lambda) are also optimized.
