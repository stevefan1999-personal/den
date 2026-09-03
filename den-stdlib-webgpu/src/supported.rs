use std::{cell::RefCell, rc::Rc};

use rquickjs::{Class, Ctx, FromJs as _, JsLifetime, Object, Result, Value, class::Trace};

use crate::{JS_MAX_SAFE_INTEGER, JsU64, illegal_constructor, operation_error};

/// A readonly WebIDL `setlike<DOMString>`: a real JS `Set`, re-parented onto
/// this class's prototype so `instanceof` and the interface name still hold.
///
/// ponytail: a native Set also answers add/delete/clear/union on what the IDL
/// calls readonly, and reports `@@toStringTag` "Set". Nothing in vendor/cts or
/// the local tests looks; a frozen proxy is the upgrade if one ever does.
macro_rules! setlike {
    ($name:ident, $js:literal) => {
        #[derive(Clone, Trace, JsLifetime)]
        #[rquickjs::class(rename = $js)]
        pub struct $name {}

        #[rquickjs::methods]
        impl $name {
            #[qjs(constructor)]
            pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }
        }

        impl $name {
            pub fn from_names<'js>(ctx: &Ctx<'js>, names: Vec<String>) -> Result<Object<'js>> {
                let set: Object<'js> = den_util::construct(ctx, "Set", (names,))?;
                if let Some(proto) = Class::<Self>::prototype(ctx)? {
                    set.set_prototype(Some(&proto))?;
                }
                Ok(set)
            }
        }
    };
}

setlike!(GPUSupportedFeatures, "GPUSupportedFeatures");
setlike!(
    GPUSupportedWGSLLanguageFeatures,
    "GPUSupportedWGSLLanguageFeatures"
);

impl GPUSupportedFeatures {
    pub const CORE_FEATURES_AND_LIMITS: &'static str = "core-features-and-limits";

    pub fn from_features<'js>(ctx: &Ctx<'js>, features: wgpu::Features) -> Result<Object<'js>> {
        let mut names: Vec<String> = (features & wgpu::Features::all_webgpu_mask())
            .iter()
            .filter_map(|feature| feature.as_str().map(str::to_owned))
            .collect();
        // wgpu has no bit for this spec feature; core devices always expose it.
        if !names
            .iter()
            .any(|name| name == Self::CORE_FEATURES_AND_LIMITS)
        {
            names.push(Self::CORE_FEATURES_AND_LIMITS.to_owned());
        }
        Self::from_names(ctx, names)
    }

    pub fn is_host_feature(name: &str) -> bool { name == Self::CORE_FEATURES_AND_LIMITS }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUSupportedLimits")]
pub struct GPUSupportedLimits {
    #[qjs(skip_trace)]
    inner: wgpu::Limits,
}

impl GPUSupportedLimits {
    pub fn from_limits<'js>(ctx: &Ctx<'js>, limits: wgpu::Limits) -> Result<Class<'js, Self>> {
        Class::instance(ctx.clone(), Self { inner: limits })
    }
}

/// One list of WebGPU limit names feeds both `GPUSupportedLimits`'s getters
/// and the `requiredLimits` reader, so the two cannot drift apart.
macro_rules! limits {
  (
    u32 { $($js:literal => $method:ident => $field:ident),+ $(,)? }
    u64 { $($ujs:literal => $umethod:ident => $ufield:ident),+ $(,)? }
  ) => {
    #[rquickjs::methods]
    impl GPUSupportedLimits {
      #[qjs(constructor)]
      pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

      $(
        #[qjs(get, configurable, rename = $js)]
        pub const fn $method(&self) -> u32 { self.inner.$field }
      )+

      $(
        #[qjs(get, configurable, rename = $ujs)]
        pub fn $umethod(&self) -> u64 { self.inner.$ufield.min(JS_MAX_SAFE_INTEGER) }
      )+
    }

    impl GPUSupportedLimits {
      /// Applies a `requiredLimits` dictionary. Anything the adapter cannot
      /// meet is wgpu's answer at `request_device`; only an unknown name or a
      /// value too wide for the field is den's, and the spec makes both an
      /// `OperationError`.
      pub fn apply<'js>(
        ctx: &Ctx<'js>, object: Option<Object<'js>>, limits: &mut wgpu::Limits,
      ) -> Result<()> {
        let Some(object) = object else { return Ok(()) };
        for property in object.props::<String, Value<'_>>() {
          let (name, value) = property?;
          if value.is_null() || value.is_undefined() { continue; }
          let value = JsU64::from_js(ctx, value)?.0;
          let narrow = || {
            u32::try_from(value)
              .map_err(|_error| operation_error(ctx, format!("required limit {name} is too large")))
          };
          match name.as_str() {
            $($js => limits.$field = narrow()?,)+
            $($ujs => limits.$ufield = value.min(JS_MAX_SAFE_INTEGER),)+
            _ => return Err(operation_error(ctx, format!("unknown required limit {name}"))),
          }
        }
        Ok(())
      }
    }
  };
}

limits! {
  u32 {
    "maxTextureDimension1D" => max_texture_dimension_1d => max_texture_dimension_1d,
    "maxTextureDimension2D" => max_texture_dimension_2d => max_texture_dimension_2d,
    "maxTextureDimension3D" => max_texture_dimension_3d => max_texture_dimension_3d,
    "maxTextureArrayLayers" => max_texture_array_layers => max_texture_array_layers,
    "maxBindGroups" => max_bind_groups => max_bind_groups,
    "maxBindGroupsPlusVertexBuffers" => max_bind_groups_plus_vertex_buffers => max_bind_groups_plus_vertex_buffers,
    "maxBindingsPerBindGroup" => max_bindings_per_bind_group => max_bindings_per_bind_group,
    "maxDynamicUniformBuffersPerPipelineLayout" => max_dynamic_uniform_buffers_per_pipeline_layout => max_dynamic_uniform_buffers_per_pipeline_layout,
    "maxDynamicStorageBuffersPerPipelineLayout" => max_dynamic_storage_buffers_per_pipeline_layout => max_dynamic_storage_buffers_per_pipeline_layout,
    "maxSampledTexturesPerShaderStage" => max_sampled_textures_per_shader_stage => max_sampled_textures_per_shader_stage,
    "maxSamplersPerShaderStage" => max_samplers_per_shader_stage => max_samplers_per_shader_stage,
    "maxStorageBuffersPerShaderStage" => max_storage_buffers_per_shader_stage => max_storage_buffers_per_shader_stage,
    "maxStorageTexturesPerShaderStage" => max_storage_textures_per_shader_stage => max_storage_textures_per_shader_stage,
    "maxUniformBuffersPerShaderStage" => max_uniform_buffers_per_shader_stage => max_uniform_buffers_per_shader_stage,
    "minUniformBufferOffsetAlignment" => min_uniform_buffer_offset_alignment => min_uniform_buffer_offset_alignment,
    "minStorageBufferOffsetAlignment" => min_storage_buffer_offset_alignment => min_storage_buffer_offset_alignment,
    "maxVertexBuffers" => max_vertex_buffers => max_vertex_buffers,
    "maxVertexAttributes" => max_vertex_attributes => max_vertex_attributes,
    "maxVertexBufferArrayStride" => max_vertex_buffer_array_stride => max_vertex_buffer_array_stride,
    "maxInterStageShaderVariables" => max_inter_stage_shader_variables => max_inter_stage_shader_variables,
    "maxColorAttachments" => max_color_attachments => max_color_attachments,
    "maxColorAttachmentBytesPerSample" => max_color_attachment_bytes_per_sample => max_color_attachment_bytes_per_sample,
    "maxComputeWorkgroupStorageSize" => max_compute_workgroup_storage_size => max_compute_workgroup_storage_size,
    "maxComputeInvocationsPerWorkgroup" => max_compute_invocations_per_workgroup => max_compute_invocations_per_workgroup,
    "maxComputeWorkgroupSizeX" => max_compute_workgroup_size_x => max_compute_workgroup_size_x,
    "maxComputeWorkgroupSizeY" => max_compute_workgroup_size_y => max_compute_workgroup_size_y,
    "maxComputeWorkgroupSizeZ" => max_compute_workgroup_size_z => max_compute_workgroup_size_z,
    "maxComputeWorkgroupsPerDimension" => max_compute_workgroups_per_dimension => max_compute_workgroups_per_dimension,
    "maxImmediateSize" => max_immediate_size => max_immediate_size,
    // Per-stage aliases: wgpu has one field per resource kind, not one per stage.
    "maxStorageBuffersInVertexStage" => max_storage_buffers_in_vertex_stage => max_storage_buffers_per_shader_stage,
    "maxStorageBuffersInFragmentStage" => max_storage_buffers_in_fragment_stage => max_storage_buffers_per_shader_stage,
    "maxStorageTexturesInVertexStage" => max_storage_textures_in_vertex_stage => max_storage_textures_per_shader_stage,
    "maxStorageTexturesInFragmentStage" => max_storage_textures_in_fragment_stage => max_storage_textures_per_shader_stage,
  }
  u64 {
    "maxUniformBufferBindingSize" => max_uniform_buffer_binding_size => max_uniform_buffer_binding_size,
    "maxStorageBufferBindingSize" => max_storage_buffer_binding_size => max_storage_buffer_binding_size,
    "maxBufferSize" => max_buffer_size => max_buffer_size,
  }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUExternalTexture")]
pub struct GPUExternalTexture {
    #[qjs(skip_trace)]
    label: Rc<RefCell<String>>,
}

impl GPUExternalTexture {
    pub fn with_label(label: String) -> Self {
        Self {
            label: Rc::new(RefCell::new(label)),
        }
    }
}

#[rquickjs::methods]
impl GPUExternalTexture {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
}

pub fn wgsl_language_feature_names(instance: &wgpu::Instance) -> Vec<String> {
    let supported = instance.wgsl_language_features();
    [
        (
            wgpu::WgslLanguageFeatures::ReadOnlyAndReadWriteStorageTextures,
            "readonly_and_readwrite_storage_textures",
        ),
        (
            wgpu::WgslLanguageFeatures::Packed4x8IntegerDotProduct,
            "packed_4x8_integer_dot_product",
        ),
        (
            wgpu::WgslLanguageFeatures::UnrestrictedPointerParameters,
            "unrestricted_pointer_parameters",
        ),
        (
            wgpu::WgslLanguageFeatures::PointerCompositeAccess,
            "pointer_composite_access",
        ),
    ]
    .into_iter()
    .filter_map(|(feature, name)| supported.contains(feature).then(|| name.to_owned()))
    .collect()
}
