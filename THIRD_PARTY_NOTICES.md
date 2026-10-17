# Third-party notices

Code adapted from third-party projects is attributed here.

## regress 0.12.0

- Used in: `crates/engine-vm/vendor/regress/`, the RegExp engine of `velyr-engine-vm` (path dependency).
- License: MIT OR Apache-2.0. Copyright ridiculousfish and the regress contributors. The license texts are kept verbatim next to the code (`LICENSE-MIT`, `LICENSE-APACHE`).
- Source: the crates.io package `regress@0.12.0` (upstream https://github.com/ridiculousfish/regress, commit `aae35dee1596cc9a9caaad2425b088081c09e5d5`), copied from the local cargo registry.
- Changes (VELYR-158): an interrupt flag polled by the backtracker and an anchored (sticky) search, exposed as `Regex::exec_interruptible` (`src/classicalbacktrack.rs`, `src/api.rs`); the empty `[profile.release]` removed from `Cargo.toml`. Details in `docs/research/regexp-redos.md`.

## readable-stream 4.7.0 (and the Node.js streams it ports)

- Used in: `crates/compat-node/src/streams/` (files `internal__streams__*.js` and `stream__promises.js`), the base of `node:stream` and `stream/promises`.
- License: MIT. Copyright Node.js contributors and Joyent, Inc.
- Source: the npm package `readable-stream@4.7.0`, taken from a local package cache (no network fetch, no runtime npm dependency).
- Changes: `require()` specifiers rewritten to this runtime's flat builtin names; a header comment added to each file. Files not used: the crypto `lazy_transform`, the browser entry points and the `_stream_*` shims.

License text, verbatim from the package:

```text
Node.js is licensed for use as follows:

"""
Copyright Node.js contributors. All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to
deal in the Software without restriction, including without limitation the
rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
sell copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
IN THE SOFTWARE.
"""

This license applies to parts of Node.js originating from the
https://github.com/joyent/node repository:

"""
Copyright Joyent, Inc. and other Node contributors. All rights reserved.
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to
deal in the Software without restriction, including without limitation the
rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
sell copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
IN THE SOFTWARE.
"""
```

## @oxc-project/runtime 0.154.0 (decorator and `using` helpers)

- Used in: `crates/runtime-modules/helpers/{decorate,decorateParam,decorateMetadata,usingCtx}.js`, served as the embedded `velyr-oxc-runtime/helpers/*` modules imported by code that `velyr-tooling-typescript` transforms.
- License: MIT. Copyright (c) 2024-present VoidZero Inc. & Contributors; Copyright (c) 2023 Boshen.
- Source: the npm package `@oxc-project/runtime@0.154.0` (https://github.com/oxc-project/oxc), files `src/helpers/esm/*.js`. The `decorate*` helpers are copies of the TypeScript emit helpers (Microsoft, Apache-2.0, per the header comment of each file).
- Changes: none; the files are byte-identical and pinned by sha256 in `crates/runtime-modules/tests/embedded.rs`.

License text, verbatim from the package:

```text
MIT License

Copyright (c) 2024-present VoidZero Inc. & Contributors
Copyright (c) 2023 Boshen

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
