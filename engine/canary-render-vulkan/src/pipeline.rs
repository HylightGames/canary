// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::ffi::CString;
use std::rc::Rc;

use ash::vk;
use canary_render::{PipelineDescriptor, VertexFormat};

use crate::device::VulkanDevice;

fn vertex_format_to_vk(format: VertexFormat) -> vk::Format {
    match format {
        VertexFormat::Float32x2 => vk::Format::R32G32_SFLOAT,
        VertexFormat::Float32x3 => vk::Format::R32G32B32_SFLOAT,
    }
}

/// A graphics pipeline: two shader stages (vertex + fragment) compiled
/// from precompiled SPIR-V, a vertex input layout, and fixed-function
/// state — no blending, no depth/stencil, matching
/// [`canary_render::PipelineDescriptor`]'s scope.
///
/// Uses **dynamic** viewport/scissor state (set per render pass in
/// [`crate::encoder::VulkanCommandEncoder::begin_render_pass`], not
/// baked in here) specifically because [`canary_render::PipelineDescriptor`]
/// doesn't carry a target size — a pipeline is created independently of
/// any specific [`crate::color_target::VulkanColorTarget`], and dynamic
/// state is the standard Vulkan way to let one pipeline be reused across
/// differently sized targets rather than needing a fixed size baked in
/// at creation time.
pub struct VulkanPipeline {
    device: Rc<ash::Device>,
    pub(crate) pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
}

impl VulkanPipeline {
    pub(crate) fn new(vk_device: &VulkanDevice, desc: &PipelineDescriptor<'_>) -> Self {
        let device = &vk_device.device;

        let vs_module = create_shader_module(device, desc.vertex_shader_spirv, desc.label);
        let fs_module = create_shader_module(device, desc.fragment_shader_spirv, desc.label);

        let entry_vs = CString::new(desc.vertex_entry_point)
            .unwrap_or_else(|e| panic!("vertex entry point name had an interior NUL: {e}"));
        let entry_fs = CString::new(desc.fragment_entry_point)
            .unwrap_or_else(|e| panic!("fragment entry point name had an interior NUL: {e}"));
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vs_module)
                .name(&entry_vs),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fs_module)
                .name(&entry_fs),
        ];

        let binding_desc = vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(desc.vertex_stride)
            .input_rate(vk::VertexInputRate::VERTEX);
        let bindings = [binding_desc];
        let attr_descs: Vec<vk::VertexInputAttributeDescription> = desc
            .vertex_attributes
            .iter()
            .map(|attr| {
                vk::VertexInputAttributeDescription::default()
                    .location(attr.shader_location)
                    .binding(0)
                    .format(vertex_format_to_vk(attr.format))
                    .offset(attr.offset)
            })
            .collect();
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attr_descs);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

        // Counts must be set even though the actual viewport/scissor
        // rectangles are provided dynamically per render pass (see this
        // struct's own docs for why) -- Vulkan still wants to know how
        // many of each this pipeline expects.
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);

        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false);
        let color_blend_attachments = [color_blend_attachment];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_blend_attachments);

        let layout_ci = vk::PipelineLayoutCreateInfo::default();
        let layout =
            unsafe { device.create_pipeline_layout(&layout_ci, None) }.unwrap_or_else(|e| {
                panic!("failed to create pipeline layout for {:?}: {e}", desc.label)
            });

        let pipeline_ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .dynamic_state(&dynamic_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend)
            .layout(layout)
            .render_pass(vk_device.render_pass)
            .subpass(0);
        let pipeline_cis = [pipeline_ci];
        let pipeline = unsafe {
            device.create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_cis, None)
        }
        .unwrap_or_else(|(_, e)| panic!("failed to create pipeline {:?}: {e}", desc.label))[0];

        // Shader modules aren't needed after pipeline creation -- the
        // pipeline has already consumed and compiled them internally.
        unsafe {
            device.destroy_shader_module(vs_module, None);
            device.destroy_shader_module(fs_module, None);
        }

        Self {
            device: Rc::clone(device),
            pipeline,
            layout,
        }
    }
}

fn create_shader_module(device: &ash::Device, spirv: &[u32], label: &str) -> vk::ShaderModule {
    let ci = vk::ShaderModuleCreateInfo::default().code(spirv);
    unsafe { device.create_shader_module(&ci, None) }
        .unwrap_or_else(|e| panic!("failed to create shader module for pipeline {label:?}: {e}"))
}

impl Drop for VulkanPipeline {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
        }
    }
}
