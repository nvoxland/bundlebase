# ADR-008: No mod.rs Files

**Status:** Accepted

**Date:** December 2024 (documented retroactively)

## Context

Rust supports two ways to organize modules:

**Option 1: mod.rs files** (traditional)
```
src/
├── bundle/
│   ├── mod.rs      # Module definition
│   ├── builder.rs
│   └── state.rs
```

**Option 2: Named module files** (Rust 2018+)
```
src/
├── bundle.rs       # Module definition
├── bundle/
│   ├── builder.rs
│   └── state.rs
```

Both are valid Rust, but we need consistency.

### Alternatives Considered

**Option 1: Use mod.rs everywhere** (traditional)
- Pros: Familiar to Rust developers from pre-2018 era
- Cons: More `mod.rs` files to navigate, ambiguous filenames

**Option 2: Use named files everywhere** (chosen)
- Pros: Cleaner file structure, clearer naming, Rust 2018+ convention
- Cons: Slightly less familiar to old-school Rust developers

**Option 3: Mixed approach** (both styles)
- Pros: Flexibility
- Cons: Inconsistency, confusion about which to use

## Decision

**Use named module files (`bundle.rs`) instead of `mod.rs` files.**

### Convention

For a module with submodules:
```
src/
├── bundle.rs           # Defines bundle module and re-exports
└── bundle/
    ├── builder.rs      # Submodule
    ├── state.rs        # Submodule
    └── operations.rs   # Submodule
```

In `bundle.rs`:
```rust
// Module contents
pub struct Bundle { ... }

// Submodules
mod builder;
mod state;
mod operations;

// Re-exports
pub use builder::BundleBuilder;
pub use state::BundleState;
pub use operations::Operation;
```

### Rationale

1. **Clarity**: `bundle.rs` is clearly the bundle module (vs. generic `mod.rs`)
2. **Navigation**: Easier to find specific modules in editor file trees
3. **Modern convention**: Rust 2018+ edition standard
4. **Consistency**: One way to organize modules across codebase

## Consequences

### Positive

- **Clearer file names**: `bundle.rs` vs `bundle/mod.rs` is more descriptive
- **Better IDE support**: Editors can differentiate modules more easily
- **Modern Rust**: Follows Rust 2018+ conventions
- **Easier navigation**: Less cognitive load when scanning file tree
- **Unique names**: Each file has unique name (no multiple `mod.rs`)

### Negative

- **Migration**: Old Rust developers might expect `mod.rs`
- **Duplication**: Module name appears both as `module.rs` and `module/` directory
- **Convention change**: Different from pre-2018 Rust tutorials

### Neutral

- **Both compile**: Functionally identical to `mod.rs` approach
- **Tooling support**: Cargo and rustc support both equally
- **File count**: Same number of files, just different organization

## Examples

### Good (named files)

```
src/
├── lib.rs
├── bundle.rs
├── bundle/
│   ├── builder.rs
│   └── state.rs
├── functions.rs
├── functions/
│   ├── registry.rs
│   └── generator.rs
└── io.rs
```

### Bad (mod.rs files)

```
src/
├── lib.rs
├── bundle/
│   ├── mod.rs      # ❌ Use bundle.rs instead
│   ├── builder.rs
│   └── state.rs
└── functions/
    ├── mod.rs      # ❌ Use functions.rs instead
    ├── registry.rs
    └── generator.rs
```

## Implementation

### Project Structure

Current bundlebase structure follows this convention:

```
rust/bundlebase/src/
├── lib.rs
├── bundle.rs          # ✅ Named file
├── bundle/
│   ├── builder.rs
│   ├── facade.rs
│   └── ...
├── functions.rs       # ✅ Named file
├── functions/
│   ├── registry.rs
│   └── ...
└── io.rs             # ✅ Named file
```

**No `mod.rs` files** exist in the codebase.

### Enforcement

1. **Code review**: Reject PRs that introduce `mod.rs` files
2. **Documentation**: Document convention in development guide
3. **Linting**: Consider adding clippy rule (if available)
4. **Templates**: Provide module templates using named files

## Comparison with Other Projects

**Major Rust projects using named files**:
- ripgrep
- tokio
- serde
- actix-web (Rust 2018+)

**Projects still using mod.rs**:
- Older codebases (pre-2018 edition)
- Projects prioritizing compatibility with old Rust tutorials

**Industry trend**: New projects overwhelmingly use named files (Rust 2018+ convention).

## Migration Guide

If you see a `mod.rs` file:

1. **Rename**: `src/module/mod.rs` → `src/module.rs`
2. **Move**: Module contents from `mod.rs` to `module.rs`
3. **Keep**: Submodules stay in `module/` directory
4. **Update**: Module declarations if necessary

Example:
```bash
# Before
src/bundle/mod.rs

# After
src/bundle.rs
```

## Related Decisions

- See [ai-rules.md section 4.1](../ai-rules.md) - Enforcement in AI code generation
- See [anti-patterns.md](../anti-patterns.md#24-never-create-modrs-files) - Anti-pattern examples

## References

- Rust 2018 Edition Guide: https://doc.rust-lang.org/edition-guide/rust-2018/path-changes.html#no-more-modrs
- Rust Module System: https://doc.rust-lang.org/book/ch07-00-managing-growing-projects-with-packages-crates-and-modules.html

## Exceptions

**None**. All modules use named files. Even for single-file modules (e.g., `io.rs` with no submodules), the convention remains consistent.
