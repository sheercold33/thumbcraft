use image::{DynamicImage, ImageBuffer, Rgba};
use std::borrow::Cow;
use std::sync::{Mutex, OnceLock};

/// Minimum source image pixel count to offload downscaling to the GPU.
/// Smaller images (< 0.5 MP, e.g. CDR embedded thumbnails) resize faster on CPU
/// without paying PCIe upload/readback latency.
pub const GPU_MIN_PIXELS: u64 = 500_000;

#[derive(Debug, Clone)]
pub struct GpuAdapterInfo {
    pub name: String,
    pub backend: String,
}

const DOWNSAMPLE_WGSL: &str = r#"
struct Params {
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> src_pixels: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst_pixels: array<u32>;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.dst_w || gid.y >= params.dst_h) {
        return;
    }

    let scale_x = f32(params.src_w) / f32(params.dst_w);
    let scale_y = f32(params.src_h) / f32(params.dst_h);

    let x0 = u32(floor(f32(gid.x) * scale_x));
    let y0 = u32(floor(f32(gid.y) * scale_y));
    let x1 = min(max(u32(ceil(f32(gid.x + 1u) * scale_x)), x0 + 1u), params.src_w);
    let y1 = min(max(u32(ceil(f32(gid.y + 1u) * scale_y)), y0 + 1u), params.src_h);

    let span_x = x1 - x0;
    let span_y = y1 - y0;
    let step_x = max(1u, (span_x + 15u) / 16u);
    let step_y = max(1u, (span_y + 15u) / 16u);

    var sum = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var count: f32 = 0.0;
    var sy = y0;
    loop {
        if (sy >= y1) {
            break;
        }
        let row_off = sy * params.src_w;
        var sx = x0;
        loop {
            if (sx >= x1) {
                break;
            }
            sum = sum + unpack4x8unorm(src_pixels[row_off + sx]);
            count = count + 1.0;
            sx = sx + step_x;
        }
        sy = sy + step_y;
    }

    let avg = sum / max(count, 1.0);
    dst_pixels[gid.y * params.dst_w + gid.x] = pack4x8unorm(avg);
}
"#;

struct GpuBuffers {
    src_buf: wgpu::Buffer,
    src_cap: u64,
    dst_buf: wgpu::Buffer,
    dst_cap: u64,
    staging_buf: wgpu::Buffer,
    staging_cap: u64,
    bind_group: wgpu::BindGroup,
}

struct GpuResizer {
    info: GpuAdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    max_storage_bytes: u64,
    buffers: Mutex<Option<GpuBuffers>>,
}

static GPU_RESIZER: OnceLock<Option<GpuResizer>> = OnceLock::new();

fn get_gpu_resizer() -> Option<&'static GpuResizer> {
    GPU_RESIZER.get_or_init(init_gpu_resizer).as_ref()
}

fn init_gpu_resizer() -> Option<GpuResizer> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;

    let adapter_info = adapter.get_info();
    // Skip pure CPU software fallback adapters (e.g. Microsoft Basic Render Driver)
    if adapter_info.device_type == wgpu::DeviceType::Cpu {
        return None;
    }

    let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
    let max_storage_bytes = (limits.max_storage_buffer_binding_size as u64)
        .min(limits.max_buffer_size)
        .min(256 * 1024 * 1024);

    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("ThumbCraft GPU Compute Resizer"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::Performance,
        },
        None,
    ))
    .ok()?;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ThumbCraft Area Downscale Shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(DOWNSAMPLE_WGSL)),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ThumbCraft Downscale BGL"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ThumbCraft Downscale Pipeline Layout"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("ThumbCraft Downscale Pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });

    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ThumbCraft Downscale Uniforms"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let info = GpuAdapterInfo {
        name: adapter_info.name,
        backend: format!("{:?}", adapter_info.backend),
    };

    Some(GpuResizer {
        info,
        device,
        queue,
        pipeline,
        bind_group_layout,
        uniform_buf,
        max_storage_bytes,
        buffers: Mutex::new(None),
    })
}

impl GpuResizer {
    fn resize_rgba(
        &self,
        src_w: u32,
        src_h: u32,
        src_rgba: &[u8],
        dst_w: u32,
        dst_h: u32,
    ) -> Option<Vec<u8>> {
        let src_bytes = (src_w as u64).checked_mul(src_h as u64)?.checked_mul(4)?;
        let dst_bytes = (dst_w as u64).checked_mul(dst_h as u64)?.checked_mul(4)?;
        if src_bytes == 0
            || dst_bytes == 0
            || src_bytes > self.max_storage_bytes
            || dst_bytes > self.max_storage_bytes
            || (src_rgba.len() as u64) < src_bytes
        {
            return None;
        }

        let mut guard = self.buffers.lock().ok()?;
        let need_alloc = match guard.as_ref() {
            Some(b) => b.src_cap < src_bytes || b.dst_cap < dst_bytes || b.staging_cap < dst_bytes,
            None => true,
        };
        if need_alloc {
            let new_src_cap = guard
                .as_ref()
                .map(|b| b.src_cap)
                .unwrap_or(0)
                .max(src_bytes.next_power_of_two())
                .min(self.max_storage_bytes);
            let new_dst_cap = guard
                .as_ref()
                .map(|b| b.dst_cap)
                .unwrap_or(0)
                .max(dst_bytes.next_power_of_two())
                .min(self.max_storage_bytes);

            let src_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ThumbCraft Src RGBA"),
                size: new_src_cap,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let dst_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ThumbCraft Dst RGBA"),
                size: new_dst_cap,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let staging_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ThumbCraft Staging Readback"),
                size: new_dst_cap,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ThumbCraft Downscale BG"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: src_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: dst_buf.as_entire_binding(),
                    },
                ],
            });
            *guard = Some(GpuBuffers {
                src_buf,
                src_cap: new_src_cap,
                dst_buf,
                dst_cap: new_dst_cap,
                staging_buf,
                staging_cap: new_dst_cap,
                bind_group,
            });
        }

        let bufs = guard.as_ref()?;
        let mut uniform_bytes = [0u8; 16];
        uniform_bytes[0..4].copy_from_slice(&src_w.to_le_bytes());
        uniform_bytes[4..8].copy_from_slice(&src_h.to_le_bytes());
        uniform_bytes[8..12].copy_from_slice(&dst_w.to_le_bytes());
        uniform_bytes[12..16].copy_from_slice(&dst_h.to_le_bytes());

        self.queue.write_buffer(&self.uniform_buf, 0, &uniform_bytes);
        self.queue
            .write_buffer(&bufs.src_buf, 0, &src_rgba[..src_bytes as usize]);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ThumbCraft Downscale Encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ThumbCraft Downscale Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bufs.bind_group, &[]);
            pass.dispatch_workgroups(dst_w.div_ceil(16), dst_h.div_ceil(16), 1);
        }
        encoder.copy_buffer_to_buffer(&bufs.dst_buf, 0, &bufs.staging_buf, 0, dst_bytes);
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = bufs.staging_buf.slice(0..dst_bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        self.device.poll(wgpu::Maintain::Wait);

        let mapped_ok = rx.recv().ok().and_then(|r| r.ok()).is_some();
        if !mapped_ok {
            return None;
        }
        let out = {
            let view = slice.get_mapped_range();
            view.to_vec()
        };
        bufs.staging_buf.unmap();
        Some(out)
    }
}

/// Returns information about the detected GPU compute adapter, if available.
pub fn gpu_adapter_info() -> Option<GpuAdapterInfo> {
    get_gpu_resizer().map(|g| g.info.clone())
}

/// Computes aspect-ratio-preserving target dimensions fitting inside `max_dim x max_dim`.
pub fn fit_thumbnail_dims(src_w: u32, src_h: u32, max_dim: u32) -> (u32, u32) {
    let max_d = max_dim.max(1);
    if src_w == 0 || src_h == 0 || (src_w <= max_d && src_h <= max_d) {
        return (src_w.max(1), src_h.max(1));
    }
    let ratio = (max_d as f64 / src_w as f64).min(max_d as f64 / src_h as f64);
    let dst_w = ((src_w as f64 * ratio).round() as u32).clamp(1, max_d);
    let dst_h = ((src_h as f64 * ratio).round() as u32).clamp(1, max_d);
    (dst_w, dst_h)
}

/// Hybrid GPU + CPU thumbnail downscaler:
/// - Images >= `GPU_MIN_PIXELS` (0.5 MP) run on the GPU WGSL compute shader when `use_gpu` is true.
/// - Smaller images (or when GPU is disabled/unavailable) use a fast integer area-averaging CPU box filter.
pub fn resize_thumbnail_hybrid(img: DynamicImage, max_dim: u32, use_gpu: bool) -> (DynamicImage, bool) {
    let (src_w, src_h) = (img.width(), img.height());
    if src_w <= max_dim && src_h <= max_dim {
        return (img, false);
    }
    let (dst_w, dst_h) = fit_thumbnail_dims(src_w, src_h, max_dim);
    let pixels = (src_w as u64).saturating_mul(src_h as u64);

    if use_gpu && pixels >= GPU_MIN_PIXELS {
        if let Some(gpu) = get_gpu_resizer() {
            let rgba = img.to_rgba8();
            if let Some(out_raw) = gpu.resize_rgba(src_w, src_h, rgba.as_raw(), dst_w, dst_h) {
                if let Some(buf) = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(dst_w, dst_h, out_raw) {
                    return (DynamicImage::ImageRgba8(buf), true);
                }
            }
        }
    }

    (fast_cpu_area_resize(&img, dst_w, dst_h), false)
}

/// Fast pure-Rust area-averaging CPU downscaler (typically 3x-5x faster than generic Triangle resize
/// on large downscale ratios while producing crisp anti-aliased thumbnails).
pub fn fast_cpu_area_resize(img: &DynamicImage, dst_w: u32, dst_h: u32) -> DynamicImage {
    let src = img.to_rgba8();
    let (src_w, src_h) = src.dimensions();
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return img.clone();
    }
    // For mild downscales (< 2x), Triangle filter looks slightly smoother than pure box average
    if src_w < dst_w.saturating_mul(2) && src_h < dst_h.saturating_mul(2) {
        return img.resize_exact(dst_w, dst_h, image::imageops::FilterType::Triangle);
    }

    let raw = src.as_raw();
    let mut out = vec![0u8; (dst_w as usize) * (dst_h as usize) * 4];
    let scale_x = src_w as f32 / dst_w as f32;
    let scale_y = src_h as f32 / dst_h as f32;
    let stride = (src_w as usize) * 4;

    for dy in 0..dst_h {
        let y0 = (((dy as f32) * scale_y).floor() as usize).min(src_h as usize - 1);
        let y1 = ((((dy + 1) as f32) * scale_y).ceil() as usize)
            .max(y0 + 1)
            .min(src_h as usize);
        let step_y = ((y1.saturating_sub(y0) + 15) / 16).max(1);
        let dst_row_off = (dy as usize) * (dst_w as usize) * 4;

        for dx in 0..dst_w {
            let x0 = (((dx as f32) * scale_x).floor() as usize).min(src_w as usize - 1);
            let x1 = ((((dx + 1) as f32) * scale_x).ceil() as usize)
                .max(x0 + 1)
                .min(src_w as usize);
            let step_x = ((x1.saturating_sub(x0) + 15) / 16).max(1);

            let mut r_sum = 0u32;
            let mut g_sum = 0u32;
            let mut b_sum = 0u32;
            let mut a_sum = 0u32;
            let mut cnt = 0u32;

            let mut sy = y0;
            while sy < y1 {
                let row = &raw[sy * stride..];
                let mut sx = x0;
                while sx < x1 {
                    let o = sx * 4;
                    r_sum += row[o] as u32;
                    g_sum += row[o + 1] as u32;
                    b_sum += row[o + 2] as u32;
                    a_sum += row[o + 3] as u32;
                    cnt += 1;
                    sx += step_x;
                }
                sy += step_y;
            }

            let d_off = dst_row_off + (dx as usize) * 4;
            let denom = cnt.max(1);
            out[d_off] = ((r_sum + denom / 2) / denom) as u8;
            out[d_off + 1] = ((g_sum + denom / 2) / denom) as u8;
            out[d_off + 2] = ((b_sum + denom / 2) / denom) as u8;
            out[d_off + 3] = ((a_sum + denom / 2) / denom) as u8;
        }
    }

    ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(dst_w, dst_h, out)
        .map(DynamicImage::ImageRgba8)
        .unwrap_or_else(|| img.resize_exact(dst_w, dst_h, image::imageops::FilterType::Triangle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_gpu_and_cpu_resize_produces_accurate_thumbnail() {
        // 1024x768 (>= 500k pixels) exercises GPU path when a hardware GPU is available
        let mut buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(1024, 768);
        for (x, y, p) in buf.enumerate_pixels_mut() {
            let r = if x < 512 { 220 } else { 40 };
            let g = if y < 384 { 180 } else { 60 };
            *p = Rgba([r, g, 120, 255]);
        }
        let img = DynamicImage::ImageRgba8(buf);

        let (thumb_gpu, used_gpu) = resize_thumbnail_hybrid(img.clone(), 200, true);
        assert_eq!(thumb_gpu.width(), 200);
        assert_eq!(thumb_gpu.height(), 150);
        let tl = thumb_gpu.to_rgba8().get_pixel(20, 20).0;
        let br = thumb_gpu.to_rgba8().get_pixel(180, 130).0;
        assert!(tl[0] > 200 && tl[1] > 160, "top-left pixel mismatch: {tl:?}");
        assert!(br[0] < 60 && br[1] < 80, "bottom-right pixel mismatch: {br:?}");

        let (thumb_cpu, used_gpu_cpu) = resize_thumbnail_hybrid(img, 200, false);
        assert!(!used_gpu_cpu);
        assert_eq!(thumb_cpu.width(), 200);
        assert_eq!(thumb_cpu.height(), 150);

        if let Some(info) = gpu_adapter_info() {
            println!("GPU adapter active: {} ({}) | used_gpu={}", info.name, info.backend, used_gpu);
            assert!(used_gpu, "Expected GPU path to run when hardware adapter is present");
        }
    }
}
