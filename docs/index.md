# PyRust

A tiny Python-like interpreter implemented in Rust.

---

## Quick start

```bash
# REPL
cargo run

# Run a script
cargo run -- examples/demo.py
```

```python
nums = [1, 2, 3]
total = 0
for n in nums:
    total += n
print("sum", total)      # sum 6
print(nums[::-1])        # [3, 2, 1]
```

---

## Documentation

| Page | Description |
|---|---|
| [Features](features) | Language, built-in functions, type methods, math module, optimizer |
| [Limitations](limitations) | What is not yet supported |
| [Optimizer](optimizer) | Details of the 15-pass peephole pipeline |
| [Benchmarks](benchmark) | Performance comparison with CPython |
| [Profiling](profiling) | Finding hot spots with perf / valgrind / flamegraph |
