# Resource table

## Purpose

Native resources are represented by opaque runtime-managed handles.

```rust
pub struct ResourceId(u32);
```

Conceptual trait:

```rust
pub trait Resource: Send {
    fn name(&self) -> &'static str;
    fn close(&mut self);
}
```

Actual async-close design may differ.

## Metadata

Each entry tracks:
- owner isolate;
- resource kind;
- creation span;
- permission/capability origin;
- quota class;
- close state.

## Safety

A stale JS handle must fail safely and must never reference a newly allocated unrelated resource. IDs therefore need generation/version semantics or equivalent protection.

## Quotas

Resource table insertion reserves quota before exposure to JS.
