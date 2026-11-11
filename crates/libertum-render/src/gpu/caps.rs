//! GPU capability gate and adapter probe for the vello page renderer (PLAN §4.5).
//!
//! The gate is a pure function over [`GpuCaps`] (a plain data snapshot), so it is unit-testable
//! without a GPU. Every threshold below is derived from the vello 0.7 sources
//! (`vello-0.7.0/src/shaders.rs` bind layouts, `vello_shaders-0.7.0/shader/*.wgsl`), and each
//! one cites where it comes from.

use vello::wgpu;
use vello::wgpu::{Backend, DeviceType, DownlevelFlags, Features, Limits, ShaderModel};

/// vello cannot (or should not) run on this GPU. Carries a user-presentable reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuUnavailable {
    pub reason: String,
}

impl GpuUnavailable {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for GpuUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GPU page rendering unavailable: {}", self.reason)
    }
}

impl std::error::Error for GpuUnavailable {}

/// Minimum limits vello 0.7 needs, with the source of each figure.
pub mod required {
    /// `coarse` binds 5 read-only + 3 read-write storage buffers
    /// (`vello/src/shaders.rs`, `let coarse = add_shader!(coarse, ..)`). Also the WebGPU default;
    /// `Limits::downlevel_defaults()` has only 4, which Iced falls back to on weak devices.
    pub const STORAGE_BUFFERS_PER_STAGE: u32 = 8;
    /// `fine_*` writes one `Rgba8Unorm` storage texture.
    pub const STORAGE_TEXTURES_PER_STAGE: u32 = 1;
    /// `fine_*` samples the gradient ramp and image atlas (two `ImageRead` bindings).
    pub const SAMPLED_TEXTURES_PER_STAGE: u32 = 2;
    /// Every shader has at most one uniform (config) buffer.
    pub const UNIFORM_BUFFERS_PER_STAGE: u32 = 1;
    /// All vello shaders use `@group(0)` only.
    pub const BIND_GROUPS: u32 = 1;
    /// Nearly every compute shader uses `@workgroup_size(256)` (16 of 20 in `shader/*.wgsl`).
    pub const INVOCATIONS_PER_WORKGROUP: u32 = 256;
    pub const WORKGROUP_SIZE_X: u32 = 256;
    /// `fine` uses `@workgroup_size(4, 16)`.
    pub const WORKGROUP_SIZE_Y: u32 = 16;
    /// `coarse.wgsl` declares `sh_bitmaps` (8 x 256 x 4 B) plus eight `array<u32, 256>`:
    /// 16384 B, exactly the WebGPU default and `downlevel_defaults` value.
    pub const WORKGROUP_STORAGE_BYTES: u32 = 16_384;
    /// Dispatch counts scale with scene size; the WebGPU default is 65535.
    pub const WORKGROUPS_PER_DIMENSION: u32 = 65_535;
    /// Estimate, not read from vello: vello grows its bump buffers with scene complexity and
    /// retries on overflow; 128 MiB is the WebGPU default binding size and what Iced requests.
    pub const STORAGE_BINDING_BYTES: u32 = 128 * 1024 * 1024;
    /// Page textures at 2x zoom on A4 are about 1190 x 1684; 4x zoom 2380 x 3368.
    pub const TEXTURE_DIMENSION_2D: u32 = 4096;
}

/// Downlevel flags vello 0.7 needs. `COMPUTE_SHADERS`: every stage is a compute shader.
/// `INDIRECT_EXECUTION`: vello uses `Command::DispatchIndirect` (`vello/src/recording.rs`).
/// (`wgpu-types` documents that Metal on Apple1/Apple2 GPUs lacks the latter.)
pub const REQUIRED_DOWNLEVEL: DownlevelFlags =
    DownlevelFlags::COMPUTE_SHADERS.union(DownlevelFlags::INDIRECT_EXECUTION);

/// A plain snapshot of what the GPU offers. `None` means "not known" (for example when only a
/// `wgpu::Device` is at hand, as inside an Iced `Pipeline::new`, which exposes limits and
/// features and (through hal) the backend, but not the adapter's downlevel flags or device
/// type); unknown items are not held against the GPU, and the empirical init checks (shader
/// build, smoke render with readback) cover them.
#[derive(Clone, Debug)]
pub struct GpuCaps {
    pub backend: Option<Backend>,
    pub device_type: Option<DeviceType>,
    pub downlevel_flags: Option<DownlevelFlags>,
    pub shader_model: Option<ShaderModel>,
    /// Whether `Rgba8Unorm` can be a write-only storage texture on this adapter.
    pub rgba8_storage_write: Option<bool>,
    pub features: Features,
    pub limits: Limits,
}

impl GpuCaps {
    /// Everything wgpu reports for an adapter. Limits are the adapter's maximums, i.e. what a
    /// device requesting `Limits::default()` (as Iced's first attempt does) would get when the
    /// adapter meets the defaults.
    pub fn from_adapter(adapter: &wgpu::Adapter) -> Self {
        let info = adapter.get_info();
        let dl = adapter.get_downlevel_capabilities();
        let fmt = adapter.get_texture_format_features(crate::gpu::TARGET_FORMAT);
        let storage = fmt
            .allowed_usages
            .contains(wgpu::TextureUsages::STORAGE_BINDING)
            && fmt
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::STORAGE_WRITE_ONLY);
        Self {
            backend: Some(info.backend),
            device_type: Some(info.device_type),
            downlevel_flags: Some(dl.flags),
            shader_model: Some(dl.shader_model),
            rgba8_storage_write: Some(storage),
            features: adapter.features(),
            limits: adapter.limits(),
        }
    }

    /// Only what a `wgpu::Device` exposes: its granted features and limits. Use this inside
    /// Iced's `Pipeline::new`; pass [`GpuCaps::from_adapter`] instead when the adapter is known.
    ///
    /// The backend is recovered with `Device::as_hal`, which only tests which hal API the device
    /// belongs to (the guard is dropped at once); it is `None` when no hal API matches.
    pub fn from_device(device: &wgpu::Device) -> Self {
        Self {
            backend: backend_of_device(device),
            device_type: None,
            downlevel_flags: None,
            shader_model: None,
            rgba8_storage_write: None,
            features: device.features(),
            limits: device.limits(),
        }
    }

    /// True when the downlevel flags and shader model say the adapter is fully WebGPU
    /// compliant (informational; vello itself needs only [`REQUIRED_DOWNLEVEL`]).
    pub fn webgpu_compliant(&self) -> Option<bool> {
        let flags = self.downlevel_flags?;
        Some(flags.contains(DownlevelFlags::compliant()) && self.shader_model? >= ShaderModel::Sm5)
    }
}

/// Which wgpu backend `device` runs on, using `Device::as_hal` with each hal API compiled in for
/// this target (`wgpu-27.0.1/build.rs` cfg aliases: Vulkan on Windows, Linux and Android; DX12
/// on Windows; GLES on Windows, Linux and Android; Metal on Apple targets).
pub fn backend_of_device(device: &wgpu::Device) -> Option<Backend> {
    use wgpu::hal::api;
    // SAFETY: the guard returned by `as_hal` is only tested for presence and dropped at once;
    // nothing is destroyed or kept.
    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
    if unsafe { device.as_hal::<api::Vulkan>() }.is_some() {
        return Some(Backend::Vulkan);
    }
    #[cfg(windows)]
    if unsafe { device.as_hal::<api::Dx12>() }.is_some() {
        return Some(Backend::Dx12);
    }
    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
    if unsafe { device.as_hal::<api::Gles>() }.is_some() {
        return Some(Backend::Gl);
    }
    #[cfg(target_vendor = "apple")]
    if unsafe { device.as_hal::<api::Metal>() }.is_some() {
        return Some(Backend::Metal);
    }
    let _ = device;
    None
}

/// Policy knobs for the gate.
#[derive(Clone, Copy, Debug, Default)]
pub struct GatePolicy {
    /// Allow backends that are rejected by default because vello has not been proven on them
    /// (OpenGL). The empirical init smoke test still applies.
    pub allow_experimental_backends: bool,
    /// Allow software rasterisers (`DeviceType::Cpu`: WARP, llvmpipe, SwiftShader), which are
    /// slower than the CPU page renderer.
    pub allow_software: bool,
}

/// The capability gate. Returns every problem found, joined into one reason, or `Ok(())`.
///
/// Rules, in order:
/// 1. Backend must be Vulkan, DX12 or Metal. OpenGL is rejected (experimental, see
///    [`GatePolicy`]), as are Noop (does not execute compute) and BrowserWebGpu (not a native
///    desktop path we ship).
/// 2. Device type `Cpu` (software rasteriser) is rejected.
/// 3. Downlevel flags must include [`REQUIRED_DOWNLEVEL`] and the shader model must be at
///    least SM5 (SM4 lacks storage images).
/// 4. `Rgba8Unorm` must be usable as a write-only storage texture.
/// 5. Limits must meet [`required`].
pub fn check_caps(caps: &GpuCaps, policy: GatePolicy) -> Result<(), GpuUnavailable> {
    let mut problems: Vec<String> = Vec::new();

    match caps.backend {
        Some(Backend::Vulkan | Backend::Dx12 | Backend::Metal) | None => {}
        Some(Backend::Gl) if policy.allow_experimental_backends => {}
        Some(Backend::Gl) => problems.push(
            "the OpenGL backend is not enabled by default (vello ran on only one OpenGL \
             driver in testing, slower than Vulkan and without sRGB texture views)"
                .into(),
        ),
        Some(Backend::Noop) => problems.push("the no-op backend cannot run compute shaders".into()),
        Some(Backend::BrowserWebGpu) => {
            problems.push("the browser WebGPU backend is not supported".into());
        }
    }

    if caps.device_type == Some(DeviceType::Cpu) && !policy.allow_software {
        problems.push("a software rasteriser is slower than the CPU page renderer".into());
    }

    if let Some(flags) = caps.downlevel_flags {
        let missing = REQUIRED_DOWNLEVEL.difference(flags);
        if !missing.is_empty() {
            problems.push(format!("missing downlevel capabilities: {missing:?}"));
        }
    }
    if let Some(sm) = caps.shader_model
        && sm < ShaderModel::Sm5
    {
        problems.push(format!("shader model {sm:?} is below SM5"));
    }
    if caps.rgba8_storage_write == Some(false) {
        problems.push("Rgba8Unorm storage textures are not writable".into());
    }

    let l = &caps.limits;
    let mut need = |name: &str, have: u32, want: u32| {
        if have < want {
            problems.push(format!("limit {name} is {have}, vello needs {want}"));
        }
    };
    need(
        "max_storage_buffers_per_shader_stage",
        l.max_storage_buffers_per_shader_stage,
        required::STORAGE_BUFFERS_PER_STAGE,
    );
    need(
        "max_storage_textures_per_shader_stage",
        l.max_storage_textures_per_shader_stage,
        required::STORAGE_TEXTURES_PER_STAGE,
    );
    need(
        "max_sampled_textures_per_shader_stage",
        l.max_sampled_textures_per_shader_stage,
        required::SAMPLED_TEXTURES_PER_STAGE,
    );
    need(
        "max_uniform_buffers_per_shader_stage",
        l.max_uniform_buffers_per_shader_stage,
        required::UNIFORM_BUFFERS_PER_STAGE,
    );
    need("max_bind_groups", l.max_bind_groups, required::BIND_GROUPS);
    need(
        "max_compute_invocations_per_workgroup",
        l.max_compute_invocations_per_workgroup,
        required::INVOCATIONS_PER_WORKGROUP,
    );
    need(
        "max_compute_workgroup_size_x",
        l.max_compute_workgroup_size_x,
        required::WORKGROUP_SIZE_X,
    );
    need(
        "max_compute_workgroup_size_y",
        l.max_compute_workgroup_size_y,
        required::WORKGROUP_SIZE_Y,
    );
    need(
        "max_compute_workgroup_storage_size",
        l.max_compute_workgroup_storage_size,
        required::WORKGROUP_STORAGE_BYTES,
    );
    need(
        "max_compute_workgroups_per_dimension",
        l.max_compute_workgroups_per_dimension,
        required::WORKGROUPS_PER_DIMENSION,
    );
    need(
        "max_storage_buffer_binding_size",
        l.max_storage_buffer_binding_size,
        required::STORAGE_BINDING_BYTES,
    );
    need(
        "max_texture_dimension_2d",
        l.max_texture_dimension_2d,
        required::TEXTURE_DIMENSION_2D,
    );

    if problems.is_empty() {
        Ok(())
    } else {
        Err(GpuUnavailable::new(problems.join("; ")))
    }
}

/// The format to create the *sampled view* of a page texture with, given the format of the
/// render target the blit draws into (Iced's surface format).
///
/// The page texture is always `Rgba8Unorm` and holds sRGB-encoded values (vello writes them
/// unconverted). When the target is an sRGB format the hardware encodes on write, so the page
/// must be sampled through an `Rgba8UnormSrgb` view (decode on read, encode on write: identity).
/// When the target is not sRGB (Iced with `web-colors`, or `Rgba16Float`/`Rgb10a2Unorm` fallbacks
/// chosen by `iced_wgpu` compositor.rs when no sRGB format exists) the raw `Rgba8Unorm` view is
/// right. Channel order of the target (`Bgra8*` vs `Rgba8*`) is irrelevant: the blit shader
/// samples and outputs RGBA, and the hardware swizzles on write.
pub fn page_view_format(target: wgpu::TextureFormat) -> wgpu::TextureFormat {
    if target.is_srgb() {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    }
}

/// What the Settings panel needs to show about one adapter.
#[derive(Clone, Debug)]
pub struct GpuReport {
    pub backend: Backend,
    pub adapter_name: String,
    pub device_type: DeviceType,
    pub driver: String,
    pub driver_info: String,
    pub vendor: u32,
    pub device: u32,
    /// Fully WebGPU compliant per wgpu's downlevel capabilities (informational).
    pub webgpu_compliant: bool,
    /// The capability gate passed. This is static: init still runs an empirical smoke test.
    pub supported: bool,
    /// Why not, when `supported` is false.
    pub reason: Option<String>,
    /// Non-fatal remarks (for example slow first start).
    pub notes: Vec<String>,
}

impl GpuReport {
    /// One line for logs and the Settings panel.
    pub fn summary(&self) -> String {
        let status = match &self.reason {
            None => "supported".to_owned(),
            Some(r) => format!("unsupported: {r}"),
        };
        format!(
            "{} ({:?}, {:?}, driver {} {}) {status}",
            self.adapter_name, self.backend, self.device_type, self.driver, self.driver_info
        )
    }
}

/// Probes an adapter with the default [`GatePolicy`].
pub fn probe(adapter: &wgpu::Adapter) -> GpuReport {
    probe_with(adapter, GatePolicy::default())
}

/// Probes an adapter with an explicit policy.
pub fn probe_with(adapter: &wgpu::Adapter, policy: GatePolicy) -> GpuReport {
    let info = adapter.get_info();
    let caps = GpuCaps::from_adapter(adapter);
    let verdict = check_caps(&caps, policy);
    let mut notes = Vec::new();
    if info.backend == Backend::Dx12 {
        notes.push(
            "DirectX 12 compiles shaders with FXC at first start; expect several seconds in the \
             background before GPU pages are ready"
                .to_owned(),
        );
    }
    GpuReport {
        backend: info.backend,
        adapter_name: info.name,
        device_type: info.device_type,
        driver: info.driver,
        driver_info: info.driver_info,
        vendor: info.vendor,
        device: info.device,
        webgpu_compliant: caps.webgpu_compliant().unwrap_or(false),
        supported: verdict.is_ok(),
        reason: verdict.err().map(|e| e.reason),
        notes,
    }
}

/// Probes the adapter a headless instance would pick (honours `WGPU_BACKEND` and
/// `WGPU_ADAPTER_NAME`). `Err` means there is no adapter at all.
pub fn probe_default() -> Result<GpuReport, GpuUnavailable> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let adapter = pollster::block_on(wgpu::util::initialize_adapter_from_env_or_default(
        &instance, None,
    ))
    .map_err(|e| GpuUnavailable::new(format!("no GPU adapter: {e}")))?;
    Ok(probe(&adapter))
}

/// Probes every adapter on every enabled backend (honours `WGPU_BACKEND`), for a Settings list.
pub fn probe_all() -> Vec<GpuReport> {
    let desc = wgpu::InstanceDescriptor::from_env_or_default();
    let backends = desc.backends;
    let instance = wgpu::Instance::new(&desc);
    instance
        .enumerate_adapters(backends)
        .iter()
        .map(probe)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fully capable adapter, as a Vulkan/DX12 desktop GPU reports it.
    fn good() -> GpuCaps {
        GpuCaps {
            backend: Some(Backend::Vulkan),
            device_type: Some(DeviceType::IntegratedGpu),
            downlevel_flags: Some(DownlevelFlags::all()),
            shader_model: Some(ShaderModel::Sm5),
            rgba8_storage_write: Some(true),
            features: Features::empty(),
            limits: Limits::default(),
        }
    }

    fn reason(caps: &GpuCaps) -> String {
        check_caps(caps, GatePolicy::default())
            .err()
            .map(|e| e.reason)
            .unwrap_or_default()
    }

    #[test]
    fn capable_adapter_passes() {
        assert_eq!(check_caps(&good(), GatePolicy::default()), Ok(()));
        for b in [Backend::Vulkan, Backend::Dx12, Backend::Metal] {
            let mut c = good();
            c.backend = Some(b);
            assert!(check_caps(&c, GatePolicy::default()).is_ok(), "{b:?}");
        }
    }

    #[test]
    fn unknown_parts_are_not_held_against_the_device() {
        let c = GpuCaps {
            backend: None,
            device_type: None,
            downlevel_flags: None,
            shader_model: None,
            rgba8_storage_write: None,
            features: Features::empty(),
            limits: Limits::default(),
        };
        assert!(check_caps(&c, GatePolicy::default()).is_ok());
    }

    #[test]
    fn gl_noop_and_webgpu_backends_are_rejected() {
        for b in [Backend::Gl, Backend::Noop, Backend::BrowserWebGpu] {
            let mut c = good();
            c.backend = Some(b);
            assert!(reason(&c).contains("backend"), "{b:?}: {}", reason(&c));
        }
    }

    #[test]
    fn gl_can_be_allowed_explicitly_but_noop_cannot() {
        let policy = GatePolicy {
            allow_experimental_backends: true,
            ..GatePolicy::default()
        };
        let mut c = good();
        c.backend = Some(Backend::Gl);
        assert!(check_caps(&c, policy).is_ok());
        c.backend = Some(Backend::Noop);
        assert!(check_caps(&c, policy).is_err());
    }

    #[test]
    fn software_rasteriser_is_rejected_unless_allowed() {
        let mut c = good();
        c.device_type = Some(DeviceType::Cpu);
        assert!(reason(&c).contains("software"));
        let policy = GatePolicy {
            allow_software: true,
            ..GatePolicy::default()
        };
        assert!(check_caps(&c, policy).is_ok());
    }

    #[test]
    fn missing_compute_or_indirect_is_rejected() {
        let mut c = good();
        c.downlevel_flags = Some(DownlevelFlags::all() - DownlevelFlags::COMPUTE_SHADERS);
        assert!(reason(&c).contains("COMPUTE_SHADERS"));
        c.downlevel_flags = Some(DownlevelFlags::all() - DownlevelFlags::INDIRECT_EXECUTION);
        assert!(reason(&c).contains("INDIRECT_EXECUTION"));
    }

    #[test]
    fn non_compliance_alone_is_not_fatal_but_is_reported() {
        // A GL-like adapter lacking VIEW_FORMATS is not WebGPU compliant, yet vello does not
        // need that flag (the page texture only lists a view format when the device allows it).
        let mut c = good();
        c.downlevel_flags = Some(DownlevelFlags::all() - DownlevelFlags::VIEW_FORMATS);
        assert!(check_caps(&c, GatePolicy::default()).is_ok());
        assert_eq!(c.webgpu_compliant(), Some(false));
        assert_eq!(good().webgpu_compliant(), Some(true));
    }

    #[test]
    fn old_shader_model_is_rejected() {
        let mut c = good();
        c.shader_model = Some(ShaderModel::Sm4);
        assert!(reason(&c).contains("SM5"));
    }

    #[test]
    fn unwritable_rgba8_storage_is_rejected() {
        let mut c = good();
        c.rgba8_storage_write = Some(false);
        assert!(reason(&c).contains("storage"));
    }

    #[test]
    fn downlevel_default_limits_are_rejected() {
        // Iced falls back to `Limits::downlevel_defaults()` when the first request fails;
        // those have 4 storage buffers per stage, below vello's 8.
        let mut c = good();
        c.limits = Limits::downlevel_defaults();
        let r = reason(&c);
        assert!(r.contains("max_storage_buffers_per_shader_stage"), "{r}");
    }

    #[test]
    fn each_limit_is_checked_and_all_problems_are_listed() {
        let mut c = good();
        c.limits.max_compute_invocations_per_workgroup = 128;
        c.limits.max_compute_workgroup_storage_size = 8192;
        c.limits.max_storage_textures_per_shader_stage = 0;
        let r = reason(&c);
        assert!(r.contains("max_compute_invocations_per_workgroup"));
        assert!(r.contains("max_compute_workgroup_storage_size"));
        assert!(r.contains("max_storage_textures_per_shader_stage"));
        assert!(!r.contains("max_bind_groups"));
    }

    #[test]
    fn webgpu_default_limits_meet_the_requirements() {
        // The requirements must never exceed the WebGPU defaults, or vello would be refused on
        // every compliant device.
        let d = Limits::default();
        assert!(d.max_storage_buffers_per_shader_stage >= required::STORAGE_BUFFERS_PER_STAGE);
        assert!(d.max_compute_workgroup_storage_size >= required::WORKGROUP_STORAGE_BYTES);
        assert!(d.max_storage_buffer_binding_size >= required::STORAGE_BINDING_BYTES);
        assert!(d.max_texture_dimension_2d >= required::TEXTURE_DIMENSION_2D);
    }

    #[test]
    fn page_view_format_follows_target_srgb_ness_not_channel_order() {
        use wgpu::TextureFormat as F;
        assert_eq!(page_view_format(F::Bgra8UnormSrgb), F::Rgba8UnormSrgb);
        assert_eq!(page_view_format(F::Rgba8UnormSrgb), F::Rgba8UnormSrgb);
        assert_eq!(page_view_format(F::Bgra8Unorm), F::Rgba8Unorm);
        assert_eq!(page_view_format(F::Rgba8Unorm), F::Rgba8Unorm);
        assert_eq!(page_view_format(F::Rgba16Float), F::Rgba8Unorm);
        assert_eq!(page_view_format(F::Rgb10a2Unorm), F::Rgba8Unorm);
    }

    #[test]
    fn report_summary_mentions_reason() {
        let r = GpuReport {
            backend: Backend::Gl,
            adapter_name: "X".into(),
            device_type: DeviceType::Other,
            driver: "d".into(),
            driver_info: "i".into(),
            vendor: 0,
            device: 0,
            webgpu_compliant: false,
            supported: false,
            reason: Some("nope".into()),
            notes: vec![],
        };
        assert!(r.summary().contains("unsupported: nope"));
    }
}
