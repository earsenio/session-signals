# Third-Party Licenses

Session Signals is distributed under the MIT License (see `LICENSE`). It
bundles and depends on third-party components under their own permissive
licenses. **No dependency requires replacement** — a full transitive audit
(Rust `cargo metadata`: ~498 crates; npm production tree) found **no
GPL-only / AGPL / SSPL / CDDL / EPL** anywhere. Everything below is
MIT-compatible for redistribution inside an MIT-licensed application.

This file collects the attribution notices that must travel with the
distribution.

---

## Bundled font — Geist Sans & Geist Mono (SIL OFL-1.1)

Session Signals bundles the Geist typeface (via the `@fontsource/geist-sans` and
`@fontsource/geist-mono` npm packages). The npm *packages* are MIT, but the
**font files themselves are licensed under the SIL Open Font License v1.1**,
which requires its copyright + license notice to be redistributed with the
fonts. Reserved Font Name: **Geist**.

> Geist Sans and Geist Mono Font
> (C) 2023 Vercel, made in collaboration with basement.studio

```
-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION AND CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

---

## Notable dependency licenses (no action required)

These are MIT-compatible and need no replacement; listed for transparency.

| Component(s) | License | Note |
|---|---|---|
| React, Tauri, serde, tiny_http, and the bulk of the tree | `MIT` / `MIT OR Apache-2.0` | Permissive; elect MIT. |
| TypeScript | `Apache-2.0` | Build-time only. |
| `cssparser`, `cssparser-macros`, `dtoa-short`, `option-ext`, `selectors` | `MPL-2.0` | File-level weak copyleft; unmodified transitive deps — compatible with MIT distribution. |
| `r-efi` | `MIT OR Apache-2.0 OR LGPL-2.1-or-later` | OR triple-license — **elect MIT**. UEFI-target dep; does not link on macOS/Windows. |
| `target-lexicon` | `Apache-2.0 WITH LLVM-exception` | More permissive than plain Apache-2.0. |
| 18 × `unicode-*` / ICU data crates | `Unicode-3.0` | Permissive, attribution-style. |

To regenerate / verify this inventory:

```
# Rust
cargo install cargo-deny && cargo deny check licenses
# npm
npx license-checker --summary --production
```
