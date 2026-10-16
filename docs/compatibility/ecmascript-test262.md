# ECMAScript and Test262 strategy

## Contract

ECMA-262 defines language semantics. Velyr's native engine must target spec behavior rather than emulate V8 implementation details.

## Test262

Test262 is the primary conformance corpus.

CI should classify tests:
- pass;
- expected fail;
- unexpected fail;
- unexpected pass;
- timeout;
- crash.

Unexpected pass is important because it means the known-failure manifest can shrink.

## Feature progression

Initial priorities:
1. lexical grammar/parser;
2. primitives/operators/control flow;
3. functions/closures;
4. objects/prototypes/descriptors;
5. errors;
6. modules;
7. Promise/jobs;
8. async/generators;
9. TypedArray/ArrayBuffer;
10. Proxy/Reflect;
11. RegExp;
12. BigInt;
13. Intl/ECMA-402;
14. newer stage-4 additions as adopted.

## Differential tests

For semantic failures:
```text
fixture.js
expected result
Velyr result
reference result
spec section
```

Never resolve a mismatch by matching a reference engine when the reference contradicts normative specification.
