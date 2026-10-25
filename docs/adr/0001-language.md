# ADR 0001: Rust (wgpu + winit) as the implementation language

Status: accepted, 2026-10-25

## Context
Primary target is the browser via WebAssembly, developed and tested in a cloud sandbox with no GPU. Native Mac/Linux and later iOS/Android are wanted. Most code will be written by autonomous agents with nobody stepping through crashes in a debugger. A later hand port to an exotic platform (an old console) is a possibility. Go was excluded (GC pauses, weak WASM/GPU story). Zig is philosophically ideal but pre-1.0.

## Options considered
| Dimension | C++20 + sokol + Emscripten + CMake | Rust + wgpu + winit + cargo |
|---|---|---|
| WASM build | Mature; smallest binaries (0.3–1 MB) | Mature; 1.5–3 MB brotli |
| GPU coverage | sokol: Metal/GL/D3D11/WebGPU/WebGL2 | wgpu: WebGPU-native; Metal/Vulkan/DX12/GLES/WebGL2 |
| Agent-written code | UB and use-after-free are the dominant failure mode; surfaces as "crashes after 10 min in Safari" | Compiler rejects memory, lifetime and data-race bugs before CI |
| Build/deps | CMake + Emscripten toolchain + vcpkg/vendoring | cargo: one command for native, wasm, test, lint |
| Seed determinism | Needs `-ffp-contract=off` discipline (clang contracts to FMA on ARM) | No contraction by default |
| Native/mobile | Path of least resistance | Desktop trivial; mobile = static lib + thin Swift/Kotlin shell |
| Ecosystem | Deep (Jolt, ImGui, miniaudio, stb) | Thinner; we build physics, meshes and world ourselves anyway |
| Compile speed | Fastest | 1–2 min clean, seconds incremental |
| Performance | Equal | Equal |

## Decision
Rust. Decisive factors: compiler-checked correctness for code nobody debugs by hand, one zero-churn toolchain for web and native, a WebGPU-first graphics API, deterministic floats by default.

## Consequences and hedges
- Simulation crates (`core`, `place`, `world`, `vehicle`, `traffic`) are `no_std` + `alloc`, written as plain structs and loops so a C port is a transliteration.
- A small internal render-hardware-interface trait sits in front of wgpu.
- Memory budgets are fixed up front (pools, arenas), console-style.
- All data formats are language-neutral with documented layouts.
- If C++ were chosen later: C++20, sokol_gfx/app/audio, Emscripten, CMake, glm, stb, cgltf, ASan/UBSan, `-Werror`, `-ffp-contract=off`.
