# Third-party notices

Source vendored or ported into this repository, beyond the crates resolved by
Cargo (whose licences are enforced by `deny.toml`).

## pi (`pi-mono`) — MIT

`crates/config/src/device_code.rs` is a Rust port of pi's OAuth device-code
polling loop and verification-URI check:

- `packages/ai/src/auth/oauth/device-code.ts` (`pollOAuthDeviceCodeFlow`,
  the RFC 8628 polling behaviours)
- `packages/ai/src/auth/oauth/xai.ts` (`validateVerificationUri`)

Upstream: <https://github.com/badlogic/pi-mono>

```
MIT License

Copyright (c) 2025 Mario Zechner

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
