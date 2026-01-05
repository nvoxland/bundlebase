# ADR-001: Rust Core Library

**Status:** Accepted

**Date:** December 2024 (documented retroactively)

## Context

Bundlebase needed a high-performance foundation for data processing that could:
- Handle large datasets efficiently (larger than RAM)
- Provide type safety and memory safety guarantees
- Enable Python integration for ease of use
- Support multi-platform deployment

## Decision

**Implement the core library in Rust with thin Python bindings via PyO3.**

### Implementation Approach

- **Core logic** lives in `rust/bundlebase/` crate (pure Rust, no Python dependencies)
- **Python bindings** in `rust/bundlebase-python/` crate (PyO3 wrapper layer)
- **Python package** in `python/` provides high-level API and convenience functions

This creates a clear separation:
1. Rust implements all business logic, data processing, and SQL execution
2. PyO3 provides the FFI bridge with async/await support
3. Python layer adds Pythonic conveniences (sync API, helper functions)

### Key Technical Details

- Rust 2021 edition
- PyO3 v0.23 for Python bindings
- Maturin v1.10+ for building Python wheels
- Support Python 3.13+

## Consequences

### Positive

- **Performance**: Near-C++ performance for data processing operations
- **Memory safety**: Rust's ownership system prevents entire classes of bugs (no segfaults, no use-after-free)
- **Type safety**: Compile-time guarantees catch errors early
- **Fearless concurrency**: Rust's type system enables safe parallel processing
- **PyO3 ecosystem**: Excellent Python FFI with async/await, zero-copy data sharing via Arrow
- **Multi-language support**: Core Rust library enables bindings for other languages in future (Node.js, R, etc.)

### Negative

- **Learning curve**: Contributors need to learn Rust (steeper than Python or Go)
- **Compile times**: Rust compilation slower than interpreted languages
- **Ecosystem size**: Fewer libraries available compared to Python or C++
- **FFI complexity**: Crossing Rust/Python boundary requires careful design (error handling, async bridges)
- **Debug experience**: Debugging across FFI boundary more complex than pure-language code

### Neutral

- **Development velocity**: Slower initial development, faster long-term (fewer runtime bugs)
- **Binary size**: Larger than pure Python, but includes entire runtime (no Python C dependencies)
- **Tooling**: Excellent Rust tooling (Cargo, Clippy, rustfmt) but different from Python ecosystem

## Related Decisions

- [ADR-002](002-datafusion-arrow.md) - Choice of DataFusion/Arrow builds on Rust decision
- [ADR-003](003-streaming-only.md) - Streaming architecture leverages Rust's zero-cost abstractions
- [ADR-007](007-no-unwrap.md) - Rust's type system enables strict error handling

## References

- **PyO3 Documentation**: https://pyo3.rs/
- **Maturin Build Tool**: https://www.maturin.rs/
- **See also**: [python-bindings.md](../python-bindings.md) for implementation details
