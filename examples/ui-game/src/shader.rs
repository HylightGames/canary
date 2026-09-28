// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! One shared WGSL → SPIR-V recipe: the scene shader and the UI paint
//! shader both compile through it. Same `naga` path (parse, validate,
//! cross-compile one stage) as the backend's own proof harnesses —
//! [`PipelineDescriptor`](canary_render::PipelineDescriptor) takes
//! precompiled SPIR-V, not WGSL, so every consumer repeats this small
//! function rather than the engine taking a shader-compiler dependency.

/// Compiles one shader stage of `source` to SPIR-V words.
pub fn compile_stage(source: &str, stage: naga::ShaderStage) -> Vec<u32> {
    let module = naga::front::wgsl::parse_str(source).expect("WGSL parses");
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    let info = validator.validate(&module).expect("WGSL validates");
    let entry_point = match stage {
        naga::ShaderStage::Vertex => "vs_main",
        naga::ShaderStage::Fragment => "fs_main",
        // Wildcard, not an exhaustive variant list: naga grows
        // `ShaderStage` over majors; this sample only ever compiles
        // vertex + fragment.
        _ => unreachable!("only vertex/fragment stages in this sample's shaders"),
    };
    let options = naga::back::spv::Options {
        lang_version: (1, 0),
        ..Default::default()
    };
    let pipeline_options = naga::back::spv::PipelineOptions {
        shader_stage: stage,
        entry_point: entry_point.to_string(),
    };
    let mut buffer = Vec::new();
    naga::back::spv::Writer::new(&options)
        .expect("SPIR-V writer constructs")
        .write(&module, &info, Some(&pipeline_options), &None, &mut buffer)
        .expect("SPIR-V writes");
    buffer
}
