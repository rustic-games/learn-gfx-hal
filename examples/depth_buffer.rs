#![allow(clippy::len_zero)]
#![allow(clippy::many_single_char_names)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

#[cfg(feature = "dx12")]
use gfx_backend_dx12 as back;
#[cfg(feature = "metal")]
use gfx_backend_metal as back;
#[cfg(feature = "vulkan")]
use gfx_backend_vulkan as back;

use arrayvec::ArrayVec;
use core::{
    marker::PhantomData,
    mem::{size_of, size_of_val, ManuallyDrop},
    ops::Deref,
};
use gfx_hal::{
    adapter::{Adapter, MemoryTypeId, PhysicalDevice},
    buffer::{IndexBufferView, Usage as BufferUsage},
    command::{ClearColor, ClearDepthStencil, ClearValue, CommandBuffer, MultiShot, Primary},
    device::Device,
    format::{Aspects, ChannelType, Format, Swizzle},
    image::{Access as ImageAccess, Layout, SubresourceRange, Usage, ViewKind},
    memory::{Pod, Properties, Requirements},
    pass::{
        Attachment, AttachmentLoadOp, AttachmentOps, AttachmentStoreOp, Subpass, SubpassDependency,
        SubpassDesc, SubpassRef,
    },
    pool::{CommandPool, CommandPoolCreateFlags},
    pso::{
        AttributeDesc, BakedStates, BasePipeline, BlendDesc, BlendOp, BlendState, ColorBlendDesc,
        ColorMask, DepthStencilDesc, DepthTest, DescriptorSetLayoutBinding, ElemOffset, ElemStride,
        Element, EntryPoint, Face, Factor, FrontFace, GraphicsPipelineDesc, GraphicsShaderSet,
        InputAssemblerDesc, LogicOp, PipelineCreationFlags, PipelineStage, PolygonMode, Rasterizer,
        Rect, ShaderStageFlags, Specialization, StencilTest, VertexBufferDesc, Viewport,
    },
    queue::{
        capability::{Capability, Supports, Transfer},
        family::QueueGroup,
        CommandQueue, Submission,
    },
    window::{Backbuffer, Extent2D, FrameSync, PresentMode, Swapchain, SwapchainConfig},
    Backend, DescriptorPool, Gpu, Graphics, IndexType, Instance, Primitive, QueueFamily, Surface,
};
use nalgebra_glm as glm;
use std::{collections::HashSet, time::Instant};
use winit::{
    dpi::LogicalSize, CreationError, DeviceEvent, ElementState, Event, EventsLoop, KeyboardInput,
    MouseButton, VirtualKeyCode, Window, WindowBuilder, WindowEvent,
};

pub const WINDOW_NAME: &str = "Depth Buffer";

pub const VERTEX_SOURCE: &str = "#version 450
layout (push_constant) uniform PushConsts {
  mat4 mvp;
} push;

layout (location = 0) in vec3 position;
layout (location = 1) in vec2 vert_uv;

layout (location = 0) out gl_PerVertex {
  vec4 gl_Position;
};
layout (location = 1) out vec2 frag_uv;

void main()
{
  gl_Position = push.mvp * vec4(position, 1.0);
  frag_uv = vert_uv;
}";

pub const FRAGMENT_SOURCE: &str = "#version 450
layout(set = 0, binding = 0) uniform texture2D tex;
layout(set = 0, binding = 1) uniform sampler samp;

layout (location = 1) in vec2 frag_uv;

layout (location = 0) out vec4 color;

void main()
{
  color = texture(sampler2D(tex, samp), frag_uv);
}";

pub static CREATURE_BYTES: &[u8] = include_bytes!("creature.png");

/// DO NOT USE THE VERSION OF THIS FUNCTION THAT'S IN THE GFX-HAL CRATE.
///
/// It can trigger UB if you upcast from a low alignment to a higher alignment
/// type. You'll be sad.
pub fn cast_slice<T: Pod, U: Pod>(ts: &[T]) -> Option<&[U]> {
    use core::mem::align_of;
    // Handle ZST (this all const folds)
    if size_of::<T>() == 0 || size_of::<U>() == 0 {
        if size_of::<T>() == size_of::<U>() {
            unsafe {
                return Some(core::slice::from_raw_parts(
                    ts.as_ptr() as *const U,
                    ts.len(),
                ));
            }
        } else {
            return None;
        }
    }
    // Handle alignments (this const folds)
    if align_of::<U>() > align_of::<T>() {
        // possible mis-alignment at the new type (this is a real runtime check)
        if (ts.as_ptr() as usize) % align_of::<U>() != 0 {
            return None;
        }
    }
    if size_of::<T>() == size_of::<U>() {
        // same size, so we direct cast, keeping the old length
        unsafe {
            Some(core::slice::from_raw_parts(
                ts.as_ptr() as *const U,
                ts.len(),
            ))
        }
    } else {
        // we might have slop, which would cause us to fail
        let byte_size = size_of::<T>() * ts.len();
        let (new_count, new_overflow) = (byte_size / size_of::<U>(), byte_size % size_of::<U>());
        if new_overflow > 0 {
            return None;
        } else {
            unsafe {
                Some(core::slice::from_raw_parts(
                    ts.as_ptr() as *const U,
                    new_count,
                ))
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Vertex {
    xyz: [f32; 3],
    uv: [f32; 2],
}
impl Vertex {
    pub fn attributes() -> Vec<AttributeDesc> {
        let position_attribute = AttributeDesc {
            location: 0,
            binding: 0,
            element: Element {
                format: Format::Rgb32Float,
                offset: 0,
            },
        };
        let uv_attribute = AttributeDesc {
            location: 1,
            binding: 0,
            element: Element {
                format: Format::Rg32Float,
                offset: size_of::<[f32; 3]>() as ElemOffset,
            },
        };
        vec![position_attribute, uv_attribute]
    }
}

#[cfg_attr(rustfmt, rustfmt_skip)]
const CUBE_VERTEXES: [Vertex; 24] = [
  // Face 1 (front)
  Vertex { xyz: [0.0, 0.0, 0.0], uv: [0.0, 1.0] }, /* bottom left */
  Vertex { xyz: [0.0, 1.0, 0.0], uv: [0.0, 0.0] }, /* top left */
  Vertex { xyz: [1.0, 0.0, 0.0], uv: [1.0, 1.0] }, /* bottom right */
  Vertex { xyz: [1.0, 1.0, 0.0], uv: [1.0, 0.0] }, /* top right */
  // Face 2 (top)
  Vertex { xyz: [0.0, 1.0, 0.0], uv: [0.0, 1.0] }, /* bottom left */
  Vertex { xyz: [0.0, 1.0, 1.0], uv: [0.0, 0.0] }, /* top left */
  Vertex { xyz: [1.0, 1.0, 0.0], uv: [1.0, 1.0] }, /* bottom right */
  Vertex { xyz: [1.0, 1.0, 1.0], uv: [1.0, 0.0] }, /* top right */
  // Face 3 (back)
  Vertex { xyz: [0.0, 0.0, 1.0], uv: [0.0, 1.0] }, /* bottom left */
  Vertex { xyz: [0.0, 1.0, 1.0], uv: [0.0, 0.0] }, /* top left */
  Vertex { xyz: [1.0, 0.0, 1.0], uv: [1.0, 1.0] }, /* bottom right */
  Vertex { xyz: [1.0, 1.0, 1.0], uv: [1.0, 0.0] }, /* top right */
  // Face 4 (bottom)
  Vertex { xyz: [0.0, 0.0, 0.0], uv: [0.0, 1.0] }, /* bottom left */
  Vertex { xyz: [0.0, 0.0, 1.0], uv: [0.0, 0.0] }, /* top left */
  Vertex { xyz: [1.0, 0.0, 0.0], uv: [1.0, 1.0] }, /* bottom right */
  Vertex { xyz: [1.0, 0.0, 1.0], uv: [1.0, 0.0] }, /* top right */
  // Face 5 (left)
  Vertex { xyz: [0.0, 0.0, 1.0], uv: [0.0, 1.0] }, /* bottom left */
  Vertex { xyz: [0.0, 1.0, 1.0], uv: [0.0, 0.0] }, /* top left */
  Vertex { xyz: [0.0, 0.0, 0.0], uv: [1.0, 1.0] }, /* bottom right */
  Vertex { xyz: [0.0, 1.0, 0.0], uv: [1.0, 0.0] }, /* top right */
  // Face 6 (right)
  Vertex { xyz: [1.0, 0.0, 0.0], uv: [0.0, 1.0] }, /* bottom left */
  Vertex { xyz: [1.0, 1.0, 0.0], uv: [0.0, 0.0] }, /* top left */
  Vertex { xyz: [1.0, 0.0, 1.0], uv: [1.0, 1.0] }, /* bottom right */
  Vertex { xyz: [1.0, 1.0, 1.0], uv: [1.0, 0.0] }, /* top right */
];

#[cfg_attr(rustfmt, rustfmt_skip)]
const CUBE_INDEXES: [u16; 36] = [
   0,  1,  2,  2,  1,  3, // front
   4,  5,  6,  7,  6,  5, // top
  10,  9,  8,  9, 10, 11, // back
  12, 14, 13, 15, 13, 14, // bottom
  16, 17, 18, 19, 18, 17, // left
  20, 21, 22, 23, 22, 21, // right
];

pub struct BufferBundle<B: Backend, D: Device<B>> {
    pub buffer: ManuallyDrop<B::Buffer>,
    pub requirements: Requirements,
    pub memory: ManuallyDrop<B::Memory>,
    pub phantom: PhantomData<D>,
}
impl<B: Backend, D: Device<B>> BufferBundle<B, D> {
    pub fn new(
        adapter: &Adapter<B>, device: &D, size: usize, usage: BufferUsage,
    ) -> Result<Self, &'static str> {
        unsafe {
            let mut buffer = device
                .create_buffer(size as u64, usage)
                .map_err(|_| "Couldn't create a buffer!")?;
            let requirements = device.get_buffer_requirements(&buffer);
            let memory_type_id = adapter
                .physical_device
                .memory_properties()
                .memory_types
                .iter()
                .enumerate()
                .find(|&(id, memory_type)| {
                    requirements.type_mask & (1 << id) != 0
                        && memory_type.properties.contains(Properties::CPU_VISIBLE)
                })
                .map(|(id, _)| MemoryTypeId(id))
                .ok_or("Couldn't find a memory type to support the buffer!")?;
            let memory = device
                .allocate_memory(memory_type_id, requirements.size)
                .map_err(|_| "Couldn't allocate buffer memory!")?;
            device
                .bind_buffer_memory(&memory, 0, &mut buffer)
                .map_err(|_| "Couldn't bind the buffer memory!")?;
            Ok(Self {
                buffer: ManuallyDrop::new(buffer),
                requirements,
                memory: ManuallyDrop::new(memory),
                phantom: PhantomData,
            })
        }
    }

    pub unsafe fn manually_drop(&self, device: &D) {
        use core::ptr::read;
        device.destroy_buffer(ManuallyDrop::into_inner(read(&self.buffer)));
        device.free_memory(ManuallyDrop::into_inner(read(&self.memory)));
    }
}

/// Parts for an image that we uploaded from the CPU and use via sampler
pub struct LoadedImage<B: Backend, D: Device<B>> {
    pub image: ManuallyDrop<B::Image>,
    pub requirements: Requirements,
    pub memory: ManuallyDrop<B::Memory>,
    pub image_view: ManuallyDrop<B::ImageView>,
    pub sampler: ManuallyDrop<B::Sampler>,
    pub phantom: PhantomData<D>,
}
impl<B: Backend, D: Device<B>> LoadedImage<B, D> {
    pub fn new<C: Capability + Supports<Transfer>>(
        adapter: &Adapter<B>, device: &D, command_pool: &mut CommandPool<B, C>,
        command_queue: &mut CommandQueue<B, C>, img: image::RgbaImage,
    ) -> Result<Self, &'static str> {
        unsafe {
            // 0. First we compute some memory related values.
            let pixel_size = size_of::<image::Rgba<u8>>();
            let row_size = pixel_size * (img.width() as usize);
            let limits = adapter.physical_device.limits();
            let row_alignment_mask = limits.min_buffer_copy_pitch_alignment as u32 - 1;
            let row_pitch = ((row_size as u32 + row_alignment_mask) & !row_alignment_mask) as usize;
            debug_assert!(row_pitch as usize >= row_size);

            // 1. make a staging buffer with enough memory for the image, and a
            //    transfer_src usage
            let required_bytes = row_pitch * img.height() as usize;
            let staging_bundle =
                BufferBundle::new(&adapter, device, required_bytes, BufferUsage::TRANSFER_SRC)?;

            // 2. use mapping writer to put the image data into that buffer
            let mut writer = device
                .acquire_mapping_writer::<u8>(
                    &staging_bundle.memory,
                    0..staging_bundle.requirements.size,
                )
                .map_err(|_| "Couldn't acquire a mapping writer to the staging buffer!")?;
            for y in 0..img.height() as usize {
                let row = &(*img)[y * row_size..(y + 1) * row_size];
                let dest_base = y * row_pitch;
                writer[dest_base..dest_base + row.len()].copy_from_slice(row);
            }
            device
                .release_mapping_writer(writer)
                .map_err(|_| "Couldn't release the mapping writer to the staging buffer!")?;

            // 3. Make an image with transfer_dst and SAMPLED usage
            let mut the_image = device
                .create_image(
                    gfx_hal::image::Kind::D2(img.width(), img.height(), 1, 1),
                    1,
                    Format::Rgba8Srgb,
                    gfx_hal::image::Tiling::Optimal,
                    gfx_hal::image::Usage::TRANSFER_DST | gfx_hal::image::Usage::SAMPLED,
                    gfx_hal::image::ViewCapabilities::empty(),
                )
                .map_err(|_| "Couldn't create the image!")?;

            // 4. allocate memory for the image and bind it
            let requirements = device.get_image_requirements(&the_image);
            let memory_type_id = adapter
                .physical_device
                .memory_properties()
                .memory_types
                .iter()
                .enumerate()
                .find(|&(id, memory_type)| {
                    // BIG NOTE: THIS IS DEVICE LOCAL NOT CPU VISIBLE
                    requirements.type_mask & (1 << id) != 0
                        && memory_type.properties.contains(Properties::DEVICE_LOCAL)
                })
                .map(|(id, _)| MemoryTypeId(id))
                .ok_or("Couldn't find a memory type to support the image!")?;
            let memory = device
                .allocate_memory(memory_type_id, requirements.size)
                .map_err(|_| "Couldn't allocate image memory!")?;
            device
                .bind_image_memory(&memory, 0, &mut the_image)
                .map_err(|_| "Couldn't bind the image memory!")?;

            // 5. create image view and sampler
            let image_view = device
                .create_image_view(
                    &the_image,
                    gfx_hal::image::ViewKind::D2,
                    Format::Rgba8Srgb,
                    gfx_hal::format::Swizzle::NO,
                    SubresourceRange {
                        aspects: Aspects::COLOR,
                        levels: 0..1,
                        layers: 0..1,
                    },
                )
                .map_err(|_| "Couldn't create the image view!")?;
            let sampler = device
                .create_sampler(gfx_hal::image::SamplerInfo::new(
                    gfx_hal::image::Filter::Nearest,
                    gfx_hal::image::WrapMode::Tile,
                ))
                .map_err(|_| "Couldn't create the sampler!")?;

            // 6. create a command buffer
            let mut cmd_buffer = command_pool.acquire_command_buffer::<gfx_hal::command::OneShot>();
            cmd_buffer.begin();

            // 7. Use a pipeline barrier to transition the image from empty/undefined
            //    to TRANSFER_WRITE/TransferDstOptimal
            let image_barrier = gfx_hal::memory::Barrier::Image {
                states: (gfx_hal::image::Access::empty(), Layout::Undefined)
                    ..(
                        gfx_hal::image::Access::TRANSFER_WRITE,
                        Layout::TransferDstOptimal,
                    ),
                target: &the_image,
                families: None,
                range: SubresourceRange {
                    aspects: Aspects::COLOR,
                    levels: 0..1,
                    layers: 0..1,
                },
            };
            cmd_buffer.pipeline_barrier(
                PipelineStage::TOP_OF_PIPE..PipelineStage::TRANSFER,
                gfx_hal::memory::Dependencies::empty(),
                &[image_barrier],
            );

            // 8. perform copy from staging buffer to image
            cmd_buffer.copy_buffer_to_image(
                &staging_bundle.buffer,
                &the_image,
                Layout::TransferDstOptimal,
                &[gfx_hal::command::BufferImageCopy {
                    buffer_offset: 0,
                    buffer_width: (row_pitch / pixel_size) as u32,
                    buffer_height: img.height(),
                    image_layers: gfx_hal::image::SubresourceLayers {
                        aspects: Aspects::COLOR,
                        level: 0,
                        layers: 0..1,
                    },
                    image_offset: gfx_hal::image::Offset { x: 0, y: 0, z: 0 },
                    image_extent: gfx_hal::image::Extent {
                        width: img.width(),
                        height: img.height(),
                        depth: 1,
                    },
                }],
            );

            // 9. use pipeline barrier to transition the image to SHADER_READ access/
            //    ShaderReadOnlyOptimal layout
            let image_barrier = gfx_hal::memory::Barrier::Image {
                states: (
                    gfx_hal::image::Access::TRANSFER_WRITE,
                    Layout::TransferDstOptimal,
                )
                    ..(
                        gfx_hal::image::Access::SHADER_READ,
                        Layout::ShaderReadOnlyOptimal,
                    ),
                target: &the_image,
                families: None,
                range: SubresourceRange {
                    aspects: Aspects::COLOR,
                    levels: 0..1,
                    layers: 0..1,
                },
            };
            cmd_buffer.pipeline_barrier(
                PipelineStage::TRANSFER..PipelineStage::FRAGMENT_SHADER,
                gfx_hal::memory::Dependencies::empty(),
                &[image_barrier],
            );

            // 10. Submit the cmd buffer to queue and wait for it
            cmd_buffer.finish();
            let upload_fence = device
                .create_fence(false)
                .map_err(|_| "Couldn't create an upload fence!")?;
            command_queue.submit_nosemaphores(Some(&cmd_buffer), Some(&upload_fence));
            device
                .wait_for_fence(&upload_fence, core::u64::MAX)
                .map_err(|_| "Couldn't wait for the fence!")?;
            device.destroy_fence(upload_fence);

            // 11. Destroy the staging bundle and one shot buffer now that we're done
            staging_bundle.manually_drop(device);
            command_pool.free(Some(cmd_buffer));

            Ok(Self {
                image: ManuallyDrop::new(the_image),
                requirements,
                memory: ManuallyDrop::new(memory),
                image_view: ManuallyDrop::new(image_view),
                sampler: ManuallyDrop::new(sampler),
                phantom: PhantomData,
            })
        }
    }

    pub unsafe fn manually_drop(&self, device: &D) {
        use core::ptr::read;
        device.destroy_sampler(ManuallyDrop::into_inner(read(&self.sampler)));
        device.destroy_image_view(ManuallyDrop::into_inner(read(&self.image_view)));
        device.destroy_image(ManuallyDrop::into_inner(read(&self.image)));
        device.free_memory(ManuallyDrop::into_inner(read(&self.memory)));
    }
}

/// Parts for a depth buffer image
pub struct DepthImage<B: Backend, D: Device<B>> {
    pub image: ManuallyDrop<B::Image>,
    pub requirements: Requirements,
    pub memory: ManuallyDrop<B::Memory>,
    pub image_view: ManuallyDrop<B::ImageView>,
    pub phantom: PhantomData<D>,
}
impl<B: Backend, D: Device<B>> DepthImage<B, D> {
    pub fn new(adapter: &Adapter<B>, device: &D, extent: Extent2D) -> Result<Self, &'static str> {
        unsafe {
            let mut the_image = device
                .create_image(
                    gfx_hal::image::Kind::D2(extent.width, extent.height, 1, 1),
                    1,
                    Format::D32Float,
                    gfx_hal::image::Tiling::Optimal,
                    gfx_hal::image::Usage::DEPTH_STENCIL_ATTACHMENT,
                    gfx_hal::image::ViewCapabilities::empty(),
                )
                .map_err(|_| "Couldn't crate the image!")?;
            let requirements = device.get_image_requirements(&the_image);
            let memory_type_id = adapter
                .physical_device
                .memory_properties()
                .memory_types
                .iter()
                .enumerate()
                .find(|&(id, memory_type)| {
                    // BIG NOTE: THIS IS DEVICE LOCAL NOT CPU VISIBLE
                    requirements.type_mask & (1 << id) != 0
                        && memory_type.properties.contains(Properties::DEVICE_LOCAL)
                })
                .map(|(id, _)| MemoryTypeId(id))
                .ok_or("Couldn't find a memory type to support the image!")?;
            let memory = device
                .allocate_memory(memory_type_id, requirements.size)
                .map_err(|_| "Couldn't allocate image memory!")?;
            device
                .bind_image_memory(&memory, 0, &mut the_image)
                .map_err(|_| "Couldn't bind the image memory!")?;
            let image_view = device
                .create_image_view(
                    &the_image,
                    gfx_hal::image::ViewKind::D2,
                    Format::D32Float,
                    gfx_hal::format::Swizzle::NO,
                    SubresourceRange {
                        aspects: Aspects::DEPTH,
                        levels: 0..1,
                        layers: 0..1,
                    },
                )
                .map_err(|_| "Couldn't create the image view!")?;
            Ok(Self {
                image: ManuallyDrop::new(the_image),
                requirements,
                memory: ManuallyDrop::new(memory),
                image_view: ManuallyDrop::new(image_view),
                phantom: PhantomData,
            })
        }
    }

    pub unsafe fn manually_drop(&self, device: &D) {
        use core::ptr::read;
        device.destroy_image_view(ManuallyDrop::into_inner(read(&self.image_view)));
        device.destroy_image(ManuallyDrop::into_inner(read(&self.image)));
        device.free_memory(ManuallyDrop::into_inner(read(&self.memory)));
    }
}

pub struct HalState {
    cube_vertices: BufferBundle<back::Backend, back::Device>,
    cube_indexes: BufferBundle<back::Backend, back::Device>,
    depth_images: Vec<DepthImage<back::Backend, back::Device>>,
    texture: LoadedImage<back::Backend, back::Device>,
    descriptor_set_layouts: Vec<<back::Backend as Backend>::DescriptorSetLayout>,
    descriptor_pool: ManuallyDrop<<back::Backend as Backend>::DescriptorPool>,
    descriptor_set: ManuallyDrop<<back::Backend as Backend>::DescriptorSet>,
    pipeline_layout: ManuallyDrop<<back::Backend as Backend>::PipelineLayout>,
    graphics_pipeline: ManuallyDrop<<back::Backend as Backend>::GraphicsPipeline>,
    current_frame: usize,
    frames_in_flight: usize,
    in_flight_fences: Vec<<back::Backend as Backend>::Fence>,
    render_finished_semaphores: Vec<<back::Backend as Backend>::Semaphore>,
    image_available_semaphores: Vec<<back::Backend as Backend>::Semaphore>,
    command_buffers: Vec<CommandBuffer<back::Backend, Graphics, MultiShot, Primary>>,
    command_pool: ManuallyDrop<CommandPool<back::Backend, Graphics>>,
    framebuffers: Vec<<back::Backend as Backend>::Framebuffer>,
    image_views: Vec<(<back::Backend as Backend>::ImageView)>,
    render_pass: ManuallyDrop<<back::Backend as Backend>::RenderPass>,
    render_area: Rect,
    queue_group: QueueGroup<back::Backend, Graphics>,
    swapchain: ManuallyDrop<<back::Backend as Backend>::Swapchain>,
    device: ManuallyDrop<back::Device>,
    _adapter: Adapter<back::Backend>,
    _surface: <back::Backend as Backend>::Surface,
    _instance: ManuallyDrop<back::Instance>,
}

impl HalState {
    /// Creates a new, fully initialized HalState.
    pub fn new(window: &Window) -> Result<Self, &'static str> {
        // Create An Instance
        let instance = back::Instance::create(WINDOW_NAME, 1);

        // Create A Surface
        let mut surface = instance.create_surface(window);

        // Select An Adapter
        let adapter = instance
            .enumerate_adapters()
            .into_iter()
            .find(|a| {
                a.queue_families
                    .iter()
                    .any(|qf| qf.supports_graphics() && surface.supports_queue_family(qf))
            })
            .ok_or("Couldn't find a graphical Adapter!")?;

        // Open A Device and take out a QueueGroup
        let (mut device, mut queue_group) = {
            let queue_family = adapter
                .queue_families
                .iter()
                .find(|qf| qf.supports_graphics() && surface.supports_queue_family(qf))
                .ok_or("Couldn't find a QueueFamily with graphics!")?;
            let Gpu { device, mut queues } = unsafe {
                adapter
                    .physical_device
                    .open(&[(&queue_family, &[1.0; 1])])
                    .map_err(|_| "Couldn't open the PhysicalDevice!")?
            };
            let queue_group = queues
                .take::<Graphics>(queue_family.id())
                .ok_or("Couldn't take ownership of the QueueGroup!")?;
            if queue_group.queues.len() > 0 {
                Ok(())
            } else {
                Err("The QueueGroup did not have any CommandQueues available!")
            }?;
            (device, queue_group)
        };

        // Create A Swapchain, this is extra long
        let (swapchain, extent, backbuffer, format, frames_in_flight) = {
            let (caps, preferred_formats, present_modes, composite_alphas) =
                surface.compatibility(&adapter.physical_device);
            info!("{:?}", caps);
            info!("Preferred Formats: {:?}", preferred_formats);
            info!("Present Modes: {:?}", present_modes);
            info!("Composite Alphas: {:?}", composite_alphas);
            //
            let present_mode = {
                use gfx_hal::window::PresentMode::*;
                [Mailbox, Fifo, Relaxed, Immediate]
                    .iter()
                    .cloned()
                    .find(|pm| present_modes.contains(pm))
                    .ok_or("No PresentMode values specified!")?
            };
            let composite_alpha = {
                use gfx_hal::window::CompositeAlpha::*;
                [Opaque, Inherit, PreMultiplied, PostMultiplied]
                    .iter()
                    .cloned()
                    .find(|ca| composite_alphas.contains(ca))
                    .ok_or("No CompositeAlpha values specified!")?
            };
            let format = match preferred_formats {
                None => Format::Rgba8Srgb,
                Some(formats) => match formats
                    .iter()
                    .find(|format| format.base_format().1 == ChannelType::Srgb)
                    .cloned()
                {
                    Some(srgb_format) => srgb_format,
                    None => formats
                        .get(0)
                        .cloned()
                        .ok_or("Preferred format list was empty!")?,
                },
            };
            let extent = {
                let window_client_area = window
                    .get_inner_size()
                    .ok_or("Window doesn't exist!")?
                    .to_physical(window.get_hidpi_factor());
                Extent2D {
                    width: caps.extents.end.width.min(window_client_area.width as u32),
                    height: caps
                        .extents
                        .end
                        .height
                        .min(window_client_area.height as u32),
                }
            };
            let image_count = if present_mode == PresentMode::Mailbox {
                (caps.image_count.end - 1).min(3)
            } else {
                (caps.image_count.end - 1).min(2)
            };
            let image_layers = 1;
            let image_usage = if caps.usage.contains(Usage::COLOR_ATTACHMENT) {
                Usage::COLOR_ATTACHMENT
            } else {
                Err("The Surface isn't capable of supporting color!")?
            };
            let swapchain_config = SwapchainConfig {
                present_mode,
                composite_alpha,
                format,
                extent,
                image_count,
                image_layers,
                image_usage,
            };
            info!("{:?}", swapchain_config);
            //
            let (swapchain, backbuffer) = unsafe {
                device
                    .create_swapchain(&mut surface, swapchain_config, None)
                    .map_err(|_| "Failed to create the swapchain!")?
            };
            (swapchain, extent, backbuffer, format, image_count as usize)
        };

        // Create Our Sync Primitives
        let (image_available_semaphores, render_finished_semaphores, in_flight_fences) = {
            let mut image_available_semaphores: Vec<<back::Backend as Backend>::Semaphore> = vec![];
            let mut render_finished_semaphores: Vec<<back::Backend as Backend>::Semaphore> = vec![];
            let mut in_flight_fences: Vec<<back::Backend as Backend>::Fence> = vec![];
            for _ in 0..frames_in_flight {
                in_flight_fences.push(
                    device
                        .create_fence(true)
                        .map_err(|_| "Could not create a fence!")?,
                );
                image_available_semaphores.push(
                    device
                        .create_semaphore()
                        .map_err(|_| "Could not create a semaphore!")?,
                );
                render_finished_semaphores.push(
                    device
                        .create_semaphore()
                        .map_err(|_| "Could not create a semaphore!")?,
                );
            }
            (
                image_available_semaphores,
                render_finished_semaphores,
                in_flight_fences,
            )
        };

        // Define A RenderPass
        let render_pass = {
            let color_attachment = Attachment {
                format: Some(format),
                samples: 1,
                ops: AttachmentOps {
                    load: AttachmentLoadOp::Clear,
                    store: AttachmentStoreOp::Store,
                },
                stencil_ops: AttachmentOps::DONT_CARE,
                layouts: Layout::Undefined..Layout::Present,
            };
            let depth_attachment = Attachment {
                format: Some(Format::D32Float),
                samples: 1,
                ops: AttachmentOps {
                    load: AttachmentLoadOp::Clear,
                    store: AttachmentStoreOp::DontCare,
                },
                stencil_ops: AttachmentOps::DONT_CARE,
                layouts: Layout::Undefined..Layout::DepthStencilAttachmentOptimal,
            };
            let subpass = SubpassDesc {
                colors: &[(0, Layout::ColorAttachmentOptimal)],
                depth_stencil: Some(&(1, Layout::DepthStencilAttachmentOptimal)),
                inputs: &[],
                resolves: &[],
                preserves: &[],
            };
            let in_dependency = SubpassDependency {
                passes: SubpassRef::External..SubpassRef::Pass(0),
                stages: PipelineStage::COLOR_ATTACHMENT_OUTPUT
                    ..PipelineStage::COLOR_ATTACHMENT_OUTPUT | PipelineStage::EARLY_FRAGMENT_TESTS,
                accesses: ImageAccess::empty()
                    ..(ImageAccess::COLOR_ATTACHMENT_READ
                        | ImageAccess::COLOR_ATTACHMENT_WRITE
                        | ImageAccess::DEPTH_STENCIL_ATTACHMENT_READ
                        | ImageAccess::DEPTH_STENCIL_ATTACHMENT_WRITE),
            };
            let out_dependency = SubpassDependency {
                passes: SubpassRef::Pass(0)..SubpassRef::External,
                stages: PipelineStage::COLOR_ATTACHMENT_OUTPUT | PipelineStage::EARLY_FRAGMENT_TESTS
                    ..PipelineStage::COLOR_ATTACHMENT_OUTPUT,
                accesses: (ImageAccess::COLOR_ATTACHMENT_READ
                    | ImageAccess::COLOR_ATTACHMENT_WRITE
                    | ImageAccess::DEPTH_STENCIL_ATTACHMENT_READ
                    | ImageAccess::DEPTH_STENCIL_ATTACHMENT_WRITE)
                    ..ImageAccess::empty(),
            };
            unsafe {
                device
                    .create_render_pass(
                        &[color_attachment, depth_attachment],
                        &[subpass],
                        &[in_dependency, out_dependency],
                    )
                    .map_err(|_| "Couldn't create a render pass!")?
            }
        };

        // Create The ImageViews
        let (image_views, depth_images, framebuffers) = match backbuffer {
            Backbuffer::Images(images) => {
                let image_views = images
                    .into_iter()
                    .map(|image| unsafe {
                        device
                            .create_image_view(
                                &image,
                                ViewKind::D2,
                                format,
                                Swizzle::NO,
                                SubresourceRange {
                                    aspects: Aspects::COLOR,
                                    levels: 0..1,
                                    layers: 0..1,
                                },
                            )
                            .map_err(|_| "Couldn't create the image_view for the image!")
                    })
                    .collect::<Result<Vec<_>, &str>>()?;
                let depth_images = image_views
                    .iter()
                    .map(|_| DepthImage::new(&adapter, &device, extent))
                    .collect::<Result<Vec<_>, &str>>()?;
                let image_extent = gfx_hal::image::Extent {
                    width: extent.width as _,
                    height: extent.height as _,
                    depth: 1,
                };
                let framebuffers = image_views
                    .iter()
                    .zip(depth_images.iter())
                    .map(|(view, depth_image)| unsafe {
                        let attachments: ArrayVec<[_; 2]> = [view, &depth_image.image_view].into();
                        device
                            .create_framebuffer(&render_pass, attachments, image_extent)
                            .map_err(|_| "Couldn't crate the framebuffer!")
                    })
                    .collect::<Result<Vec<_>, &str>>()?;
                (image_views, depth_images, framebuffers)
            }
            Backbuffer::Framebuffer(_) => unimplemented!("Can't handle framebuffer backbuffer!"),
        };

        // Create Our CommandPool
        let mut command_pool = unsafe {
            device
                .create_command_pool_typed(&queue_group, CommandPoolCreateFlags::RESET_INDIVIDUAL)
                .map_err(|_| "Could not create the raw command pool!")?
        };

        // Create Our CommandBuffers
        let command_buffers: Vec<_> = framebuffers
            .iter()
            .map(|_| command_pool.acquire_command_buffer())
            .collect();

        // Build our pipeline and vertex buffer
        let (
            descriptor_set_layouts,
            descriptor_pool,
            descriptor_set,
            pipeline_layout,
            gfx_pipeline,
        ) = Self::create_pipeline(&mut device, extent, &render_pass)?;

        let cube_vertices = BufferBundle::new(
            &adapter,
            &device,
            size_of_val(&CUBE_VERTEXES),
            BufferUsage::VERTEX,
        )?;

        // Write the vertex data just once.
        unsafe {
            let mut data_target = device
                .acquire_mapping_writer(&cube_vertices.memory, 0..cube_vertices.requirements.size)
                .map_err(|_| "Failed to acquire an index buffer mapping writer!")?;
            data_target[..CUBE_VERTEXES.len()].copy_from_slice(&CUBE_VERTEXES);
            device
                .release_mapping_writer(data_target)
                .map_err(|_| "Couldn't release the index buffer mapping writer!")?;
        }

        let cube_indexes = BufferBundle::new(
            &adapter,
            &device,
            size_of_val(&CUBE_INDEXES),
            BufferUsage::INDEX,
        )?;

        // Write the index data just once.
        unsafe {
            let mut data_target = device
                .acquire_mapping_writer(&cube_indexes.memory, 0..cube_indexes.requirements.size)
                .map_err(|_| "Failed to acquire an index buffer mapping writer!")?;
            data_target[..CUBE_INDEXES.len()].copy_from_slice(&CUBE_INDEXES);
            device
                .release_mapping_writer(data_target)
                .map_err(|_| "Couldn't release the index buffer mapping writer!")?;
        }

        let texture = LoadedImage::new(
            &adapter,
            &device,
            &mut command_pool,
            &mut queue_group.queues[0],
            image::load_from_memory(CREATURE_BYTES)
                .expect("Binary corrupted!")
                .to_rgba(),
        )?;

        unsafe {
            device.write_descriptor_sets(vec![
                gfx_hal::pso::DescriptorSetWrite {
                    set: &descriptor_set,
                    binding: 0,
                    array_offset: 0,
                    descriptors: Some(gfx_hal::pso::Descriptor::Image(
                        texture.image_view.deref(),
                        Layout::ShaderReadOnlyOptimal,
                    )),
                },
                gfx_hal::pso::DescriptorSetWrite {
                    set: &descriptor_set,
                    binding: 1,
                    array_offset: 0,
                    descriptors: Some(gfx_hal::pso::Descriptor::Sampler(texture.sampler.deref())),
                },
            ]);
        }

        Ok(Self {
            cube_vertices,
            cube_indexes,
            texture,
            depth_images,
            descriptor_pool: ManuallyDrop::new(descriptor_pool),
            descriptor_set: ManuallyDrop::new(descriptor_set),
            _instance: ManuallyDrop::new(instance),
            _surface: surface,
            _adapter: adapter,
            device: ManuallyDrop::new(device),
            queue_group,
            swapchain: ManuallyDrop::new(swapchain),
            render_area: extent.to_extent().rect(),
            render_pass: ManuallyDrop::new(render_pass),
            image_views,
            framebuffers,
            command_pool: ManuallyDrop::new(command_pool),
            command_buffers,
            image_available_semaphores,
            render_finished_semaphores,
            in_flight_fences,
            frames_in_flight,
            current_frame: 0,
            descriptor_set_layouts,
            pipeline_layout: ManuallyDrop::new(pipeline_layout),
            graphics_pipeline: ManuallyDrop::new(gfx_pipeline),
        })
    }

    #[allow(clippy::type_complexity)]
    fn create_pipeline(
        device: &mut back::Device, extent: Extent2D,
        render_pass: &<back::Backend as Backend>::RenderPass,
    ) -> Result<
        (
            Vec<<back::Backend as Backend>::DescriptorSetLayout>,
            <back::Backend as Backend>::DescriptorPool,
            <back::Backend as Backend>::DescriptorSet,
            <back::Backend as Backend>::PipelineLayout,
            <back::Backend as Backend>::GraphicsPipeline,
        ),
        &'static str,
    > {
        let mut compiler = shaderc::Compiler::new().ok_or("shaderc not found!")?;
        let vertex_compile_artifact = compiler
            .compile_into_spirv(
                VERTEX_SOURCE,
                shaderc::ShaderKind::Vertex,
                "vertex.vert",
                "main",
                None,
            )
            .map_err(|e| {
                error!("{}", e);
                "Couldn't compile vertex shader!"
            })?;
        let fragment_compile_artifact = compiler
            .compile_into_spirv(
                FRAGMENT_SOURCE,
                shaderc::ShaderKind::Fragment,
                "fragment.frag",
                "main",
                None,
            )
            .map_err(|e| {
                error!("{}", e);
                "Couldn't compile fragment shader!"
            })?;
        let vertex_shader_module = unsafe {
            device
                .create_shader_module(vertex_compile_artifact.as_binary_u8())
                .map_err(|_| "Couldn't make the vertex module")?
        };
        let fragment_shader_module = unsafe {
            device
                .create_shader_module(fragment_compile_artifact.as_binary_u8())
                .map_err(|_| "Couldn't make the fragment module")?
        };
        let (descriptor_set_layouts, descriptor_pool, descriptor_set, layout, gfx_pipeline) = {
            let (vs_entry, fs_entry) = (
                EntryPoint {
                    entry: "main",
                    module: &vertex_shader_module,
                    specialization: Specialization {
                        constants: &[],
                        data: &[],
                    },
                },
                EntryPoint {
                    entry: "main",
                    module: &fragment_shader_module,
                    specialization: Specialization {
                        constants: &[],
                        data: &[],
                    },
                },
            );
            let shaders = GraphicsShaderSet {
                vertex: vs_entry,
                hull: None,
                domain: None,
                geometry: None,
                fragment: Some(fs_entry),
            };

            let input_assembler = InputAssemblerDesc::new(Primitive::TriangleList);

            let vertex_buffers: Vec<VertexBufferDesc> = vec![VertexBufferDesc {
                binding: 0,
                stride: size_of::<Vertex>() as ElemStride,
                rate: 0,
            }];

            let attributes: Vec<AttributeDesc> = Vertex::attributes();

            let rasterizer = Rasterizer {
                depth_clamping: false,
                polygon_mode: PolygonMode::Fill,
                cull_face: Face::BACK,
                front_face: FrontFace::Clockwise,
                depth_bias: None,
                conservative: false,
            };

            let depth_stencil = DepthStencilDesc {
                depth: DepthTest::On {
                    fun: gfx_hal::pso::Comparison::LessEqual,
                    write: true,
                },
                depth_bounds: false,
                stencil: StencilTest::Off,
            };

            let blender = {
                let blend_state = BlendState::On {
                    color: BlendOp::Add {
                        src: Factor::One,
                        dst: Factor::Zero,
                    },
                    alpha: BlendOp::Add {
                        src: Factor::One,
                        dst: Factor::Zero,
                    },
                };
                BlendDesc {
                    logic_op: Some(LogicOp::Copy),
                    targets: vec![ColorBlendDesc(ColorMask::ALL, blend_state)],
                }
            };

            let baked_states = BakedStates {
                viewport: Some(Viewport {
                    rect: extent.to_extent().rect(),
                    depth: (0.0..1.0),
                }),
                scissor: Some(extent.to_extent().rect()),
                blend_color: None,
                depth_bounds: None,
            };

            let descriptor_set_layouts: Vec<<back::Backend as Backend>::DescriptorSetLayout> =
                vec![unsafe {
                    device
                        .create_descriptor_set_layout(
                            &[
                                DescriptorSetLayoutBinding {
                                    binding: 0,
                                    ty: gfx_hal::pso::DescriptorType::SampledImage,
                                    count: 1,
                                    stage_flags: ShaderStageFlags::FRAGMENT,
                                    immutable_samplers: false,
                                },
                                DescriptorSetLayoutBinding {
                                    binding: 1,
                                    ty: gfx_hal::pso::DescriptorType::Sampler,
                                    count: 1,
                                    stage_flags: ShaderStageFlags::FRAGMENT,
                                    immutable_samplers: false,
                                },
                            ],
                            &[],
                        )
                        .map_err(|_| "Couldn't make a DescriptorSetLayout")?
                }];

            let mut descriptor_pool = unsafe {
                device
                    .create_descriptor_pool(
                        1, // sets
                        &[
                            gfx_hal::pso::DescriptorRangeDesc {
                                ty: gfx_hal::pso::DescriptorType::SampledImage,
                                count: 1,
                            },
                            gfx_hal::pso::DescriptorRangeDesc {
                                ty: gfx_hal::pso::DescriptorType::Sampler,
                                count: 1,
                            },
                        ],
                    )
                    .map_err(|_| "Couldn't create a descriptor pool!")?
            };

            let descriptor_set = unsafe {
                descriptor_pool
                    .allocate_set(&descriptor_set_layouts[0])
                    .map_err(|_| "Couldn't make a Descriptor Set!")?
            };

            let push_constants = vec![(ShaderStageFlags::VERTEX, 0..16)];
            let layout = unsafe {
                device
                    .create_pipeline_layout(&descriptor_set_layouts, push_constants)
                    .map_err(|_| "Couldn't create a pipeline layout")?
            };

            let gfx_pipeline = {
                let desc = GraphicsPipelineDesc {
                    shaders,
                    rasterizer,
                    vertex_buffers,
                    attributes,
                    input_assembler,
                    blender,
                    depth_stencil,
                    multisampling: None,
                    baked_states,
                    layout: &layout,
                    subpass: Subpass {
                        index: 0,
                        main_pass: render_pass,
                    },
                    flags: PipelineCreationFlags::empty(),
                    parent: BasePipeline::None,
                };

                unsafe {
                    device.create_graphics_pipeline(&desc, None).map_err(|e| {
                        error!("{}", e);
                        "Couldn't create a graphics pipeline!"
                    })?
                }
            };

            (
                descriptor_set_layouts,
                descriptor_pool,
                descriptor_set,
                layout,
                gfx_pipeline,
            )
        };

        unsafe {
            device.destroy_shader_module(vertex_shader_module);
            device.destroy_shader_module(fragment_shader_module);
        }

        Ok((
            descriptor_set_layouts,
            descriptor_pool,
            descriptor_set,
            layout,
            gfx_pipeline,
        ))
    }

    /// Draw a frame that's just cleared to the color specified.
    pub fn draw_clear_frame(&mut self, color: [f32; 4]) -> Result<(), &'static str> {
        // SETUP FOR THIS FRAME
        let image_available = &self.image_available_semaphores[self.current_frame];
        let render_finished = &self.render_finished_semaphores[self.current_frame];
        // Advance the frame _before_ we start using the `?` operator
        self.current_frame = (self.current_frame + 1) % self.frames_in_flight;

        let (i_u32, i_usize) = unsafe {
            let image_index = self
                .swapchain
                .acquire_image(core::u64::MAX, FrameSync::Semaphore(image_available))
                .map_err(|_| "Couldn't acquire an image from the swapchain!")?;
            (image_index, image_index as usize)
        };

        let flight_fence = &self.in_flight_fences[i_usize];
        unsafe {
            self.device
                .wait_for_fence(flight_fence, core::u64::MAX)
                .map_err(|_| "Failed to wait on the fence!")?;
            self.device
                .reset_fence(flight_fence)
                .map_err(|_| "Couldn't reset the fence!")?;
        }

        // RECORD COMMANDS
        unsafe {
            let buffer = &mut self.command_buffers[i_usize];
            let clear_values = [ClearValue::Color(ClearColor::Float(color))];
            buffer.begin(false);
            buffer.begin_render_pass_inline(
                &self.render_pass,
                &self.framebuffers[i_usize],
                self.render_area,
                clear_values.iter(),
            );
            buffer.finish();
        }

        // SUBMISSION AND PRESENT
        let command_buffers = &self.command_buffers[i_usize..=i_usize];
        let wait_semaphores: ArrayVec<[_; 1]> =
            [(image_available, PipelineStage::COLOR_ATTACHMENT_OUTPUT)].into();
        let signal_semaphores: ArrayVec<[_; 1]> = [render_finished].into();
        // yes, you have to write it twice like this. yes, it's silly.
        let present_wait_semaphores: ArrayVec<[_; 1]> = [render_finished].into();
        let submission = Submission {
            command_buffers,
            wait_semaphores,
            signal_semaphores,
        };
        let the_command_queue = &mut self.queue_group.queues[0];
        unsafe {
            the_command_queue.submit(submission, Some(flight_fence));
            self.swapchain
                .present(the_command_queue, i_u32, present_wait_semaphores)
                .map_err(|_| "Failed to present into the swapchain!")
        }
    }

    /// Draws one cube per model matrix given.
    pub fn draw_cubes_frame(
        &mut self, view_projection: &glm::TMat4<f32>, models: &[glm::TMat4<f32>],
    ) -> Result<(), &'static str> {
        // SETUP FOR THIS FRAME
        let image_available = &self.image_available_semaphores[self.current_frame];
        let render_finished = &self.render_finished_semaphores[self.current_frame];
        // Advance the frame _before_ we start using the `?` operator
        self.current_frame = (self.current_frame + 1) % self.frames_in_flight;

        let (i_u32, i_usize) = unsafe {
            let image_index = self
                .swapchain
                .acquire_image(core::u64::MAX, FrameSync::Semaphore(image_available))
                .map_err(|_| "Couldn't acquire an image from the swapchain!")?;
            (image_index, image_index as usize)
        };

        let flight_fence = &self.in_flight_fences[i_usize];
        unsafe {
            self.device
                .wait_for_fence(flight_fence, core::u64::MAX)
                .map_err(|_| "Failed to wait on the fence!")?;
            self.device
                .reset_fence(flight_fence)
                .map_err(|_| "Couldn't reset the fence!")?;
        }

        // RECORD COMMANDS
        unsafe {
            let buffer = &mut self.command_buffers[i_usize];
            const CUBE_CLEAR: [ClearValue; 2] = [
                ClearValue::Color(ClearColor::Float([0.1, 0.2, 0.3, 1.0])),
                ClearValue::DepthStencil(ClearDepthStencil(1.0, 0)),
            ];
            buffer.begin(false);
            {
                let mut encoder = buffer.begin_render_pass_inline(
                    &self.render_pass,
                    &self.framebuffers[i_usize],
                    self.render_area,
                    CUBE_CLEAR.iter(),
                );
                encoder.bind_graphics_pipeline(&self.graphics_pipeline);
                encoder.bind_vertex_buffers(0, Some((self.cube_vertices.buffer.deref(), 0)));
                encoder.bind_index_buffer(IndexBufferView {
                    buffer: &self.cube_indexes.buffer,
                    offset: 0,
                    index_type: IndexType::U16,
                });
                encoder.bind_graphics_descriptor_sets(
                    &self.pipeline_layout,
                    0,
                    Some(self.descriptor_set.deref()),
                    &[],
                );
                // ONE DRAW CALL PER MODEL MATRIX WE'RE GIVEN
                for model in models.iter() {
                    let mvp = view_projection * model;
                    encoder.push_graphics_constants(
                        &self.pipeline_layout,
                        ShaderStageFlags::VERTEX,
                        0,
                        cast_slice::<f32, u32>(&mvp.data)
                            .expect("this cast never fails for same-aligned same-size data"),
                    );
                    encoder.draw_indexed(0..36, 0, 0..1);
                }
            }
            buffer.finish();
        }

        // SUBMISSION AND PRESENT
        let command_buffers = &self.command_buffers[i_usize..=i_usize];
        let wait_semaphores: ArrayVec<[_; 1]> =
            [(image_available, PipelineStage::COLOR_ATTACHMENT_OUTPUT)].into();
        let signal_semaphores: ArrayVec<[_; 1]> = [render_finished].into();
        // yes, you have to write it twice like this. yes, it's silly.
        let present_wait_semaphores: ArrayVec<[_; 1]> = [render_finished].into();
        let submission = Submission {
            command_buffers,
            wait_semaphores,
            signal_semaphores,
        };
        let the_command_queue = &mut self.queue_group.queues[0];
        unsafe {
            the_command_queue.submit(submission, Some(flight_fence));
            self.swapchain
                .present(the_command_queue, i_u32, present_wait_semaphores)
                .map_err(|_| "Failed to present into the swapchain!")
        }
    }
}

impl core::ops::Drop for HalState {
    /// We have to clean up "leaf" elements before "root" elements. Basically, we
    /// clean up in reverse of the order that we created things.
    fn drop(&mut self) {
        let _ = self.device.wait_idle();
        unsafe {
            for depth_image in self.depth_images.drain(..) {
                depth_image.manually_drop(&self.device);
            }
            for descriptor_set_layout in self.descriptor_set_layouts.drain(..) {
                self.device
                    .destroy_descriptor_set_layout(descriptor_set_layout)
            }
            for fence in self.in_flight_fences.drain(..) {
                self.device.destroy_fence(fence)
            }
            for semaphore in self.render_finished_semaphores.drain(..) {
                self.device.destroy_semaphore(semaphore)
            }
            for semaphore in self.image_available_semaphores.drain(..) {
                self.device.destroy_semaphore(semaphore)
            }
            for framebuffer in self.framebuffers.drain(..) {
                self.device.destroy_framebuffer(framebuffer);
            }
            for image_view in self.image_views.drain(..) {
                self.device.destroy_image_view(image_view);
            }
            // LAST RESORT STYLE CODE, NOT TO BE IMITATED LIGHTLY
            self.cube_vertices.manually_drop(self.device.deref());
            self.cube_indexes.manually_drop(self.device.deref());
            self.texture.manually_drop(self.device.deref());
            use core::ptr::read;
            // this implicitly frees all descriptor sets from this pool
            self.device
                .destroy_descriptor_pool(ManuallyDrop::into_inner(read(&self.descriptor_pool)));
            self.device
                .destroy_pipeline_layout(ManuallyDrop::into_inner(read(&self.pipeline_layout)));
            self.device
                .destroy_graphics_pipeline(ManuallyDrop::into_inner(read(&self.graphics_pipeline)));
            self.device.destroy_command_pool(
                ManuallyDrop::into_inner(read(&self.command_pool)).into_raw(),
            );
            self.device
                .destroy_render_pass(ManuallyDrop::into_inner(read(&self.render_pass)));
            self.device
                .destroy_swapchain(ManuallyDrop::into_inner(read(&self.swapchain)));
            ManuallyDrop::drop(&mut self.device);
            ManuallyDrop::drop(&mut self._instance);
        }
    }
}

#[derive(Debug)]
pub struct WinitState {
    pub events_loop: EventsLoop,
    pub window: Window,
    pub keys_held: HashSet<VirtualKeyCode>,
    pub grabbed: bool,
}

impl WinitState {
    /// Constructs a new `EventsLoop` and `Window` pair.
    ///
    /// The specified title and size are used, other elements are default.
    /// ## Failure
    /// It's possible for the window creation to fail. This is unlikely.
    pub fn new<T: Into<String>>(title: T, size: LogicalSize) -> Result<Self, CreationError> {
        let events_loop = EventsLoop::new();
        let output = WindowBuilder::new()
            .with_title(title)
            .with_dimensions(size)
            .build(&events_loop);
        output.map(|window| Self {
            events_loop,
            window,
            grabbed: false,
            keys_held: HashSet::new(),
        })
    }
}

impl Default for WinitState {
    /// Makes an 800x600 window with the `WINDOW_NAME` value as the title.
    /// ## Panics
    /// If a `CreationError` occurs.
    fn default() -> Self {
        Self::new(
            WINDOW_NAME,
            LogicalSize {
                width: 800.0,
                height: 600.0,
            },
        )
        .expect("Could not create a window!")
    }
}

#[derive(Debug, Clone, Default)]
pub struct UserInput {
    pub end_requested: bool,
    pub new_frame_size: Option<(f64, f64)>,
    pub swap_projection: bool,
    pub keys_held: HashSet<VirtualKeyCode>,
    pub orientation_change: (f32, f32),
    pub seconds: f32,
}

impl UserInput {
    pub fn poll_events_loop(winit_state: &mut WinitState, last_timestamp: &mut Instant) -> Self {
        let mut output = UserInput::default();
        // We have to manually split the borrow here. rustc, why you so dumb sometimes?
        let events_loop = &mut winit_state.events_loop;
        let window = &mut winit_state.window;
        let keys_held = &mut winit_state.keys_held;
        let grabbed = &mut winit_state.grabbed;
        // now we actually poll those events
        events_loop.poll_events(|event| match event {
            // Close when asked
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => output.end_requested = true,

            // Track all keys, all the time. Note that because of key rollover details
            // it's possible to get key released events for keys we don't think are
            // pressed. This is a hardware limit, not something you can evade.
            Event::DeviceEvent {
                event:
                    DeviceEvent::Key(KeyboardInput {
                        virtual_keycode: Some(code),
                        state,
                        ..
                    }),
                ..
            } => drop(match state {
                ElementState::Pressed => keys_held.insert(code),
                ElementState::Released => keys_held.remove(&code),
            }),

            // We want to respond to some of the keys specially when they're also
            // window events too (meaning that the window was focused when the event
            // happened).
            Event::WindowEvent {
                event:
                    WindowEvent::KeyboardInput {
                        input:
                            KeyboardInput {
                                state,
                                virtual_keycode: Some(code),
                                ..
                            },
                        ..
                    },
                ..
            } => {
                #[cfg(feature = "metal")]
                {
                    match state {
                        ElementState::Pressed => keys_held.insert(code),
                        ElementState::Released => keys_held.remove(&code),
                    }
                };
                if state == ElementState::Pressed {
                    match code {
                        VirtualKeyCode::Tab => output.swap_projection = !output.swap_projection,
                        VirtualKeyCode::Escape => {
                            if *grabbed {
                                debug!("Escape pressed while grabbed, releasing the mouse!");
                                window
                                    .grab_cursor(false)
                                    .expect("Failed to release the mouse grab!");
                                window.hide_cursor(false);
                                *grabbed = false;
                            }
                        }
                        _ => (),
                    }
                }
            }

            // Always track the mouse motion, but only update the orientation if
            // we're "grabbed".
            Event::DeviceEvent {
                event: DeviceEvent::MouseMotion { delta: (dx, dy) },
                ..
            } => {
                if *grabbed {
                    output.orientation_change.0 -= dx as f32;
                    output.orientation_change.1 -= dy as f32;
                }
            }

            // Left clicking in the window causes the mouse to get grabbed
            Event::WindowEvent {
                event:
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Left,
                        ..
                    },
                ..
            } => {
                if *grabbed {
                    debug!("Click! We already have the mouse grabbed.");
                } else {
                    debug!("Click! Grabbing the mouse.");
                    window.grab_cursor(true).expect("Failed to grab the mouse!");
                    window.hide_cursor(true);
                    *grabbed = true;
                }
            }

            // Automatically release the mouse when focus is lost
            Event::WindowEvent {
                event: WindowEvent::Focused(false),
                ..
            } => {
                if *grabbed {
                    debug!("Lost Focus, releasing the mouse grab...");
                    window
                        .grab_cursor(false)
                        .expect("Failed to release the mouse grab!");
                    window.hide_cursor(false);
                    *grabbed = false;
                } else {
                    debug!("Lost Focus when mouse wasn't grabbed.");
                }
            }

            // Update our size info if the window changes size.
            Event::WindowEvent {
                event: WindowEvent::Resized(logical),
                ..
            } => {
                output.new_frame_size = Some((logical.width, logical.height));
            }

            _ => (),
        });
        output.seconds = {
            let now = Instant::now();
            let duration = now.duration_since(*last_timestamp);
            *last_timestamp = now;
            duration.as_secs() as f32 + duration.subsec_nanos() as f32 * 1e-9
        };
        output.keys_held = if *grabbed {
            keys_held.clone()
        } else {
            HashSet::new()
        };
        output
    }
}

#[derive(Debug, Clone)]
pub struct LocalState {
    pub frame_width: f64,
    pub frame_height: f64,
    pub cubes: Vec<glm::TMat4<f32>>,
    pub camera: QuaternionFreeCamera,
    pub perspective_projection: glm::TMat4<f32>,
    pub orthographic_projection: glm::TMat4<f32>,
    pub is_orthographic: bool,
    pub spare_time: f32,
}

impl LocalState {
    pub fn update_from_input(&mut self, input: UserInput) {
        if let Some(frame_size) = input.new_frame_size {
            self.frame_width = frame_size.0;
            self.frame_height = frame_size.1;
        }
        if input.swap_projection {
            self.is_orthographic = !self.is_orthographic;
        }
        assert!(self.frame_width != 0.0 && self.frame_height != 0.0);
        self.spare_time += input.seconds;
        const ONE_SIXTIETH: f32 = 1.0 / 60.0;
        // do world physics if we have any spare time
        while self.spare_time > 0.0 {
            for (i, cube_mut) in self.cubes.iter_mut().enumerate() {
                let r = ONE_SIXTIETH * 30.0 * (i as f32 + 1.0);
                *cube_mut = glm::rotate(
                    &cube_mut,
                    f32::to_radians(r),
                    &glm::make_vec3(&[0.3, 0.4, 0.5]).normalize(),
                );
            }
            self.spare_time -= ONE_SIXTIETH;
        }
        // do camera updates distinctly from physics, based on this frame's time
        /* EULER CAMERA
        const MOUSE_SENSITIVITY: f32 = 0.05;
        let d_pitch_deg = input.orientation_change.1 * MOUSE_SENSITIVITY;
        let d_yaw_deg = -input.orientation_change.0 * MOUSE_SENSITIVITY;
        self.camera.update_orientation(d_pitch_deg, d_yaw_deg);
        self
          .camera
          .update_position(&input.keys_held, 5.0 * input.seconds);
        // */

        // /* FREE CAMERA
        const MOUSE_SENSITIVITY: f32 = 0.0005;
        let d_pitch = -input.orientation_change.1 * MOUSE_SENSITIVITY;
        let d_yaw = -input.orientation_change.0 * MOUSE_SENSITIVITY;
        let mut d_roll = 0.0;
        if input.keys_held.contains(&VirtualKeyCode::Z) {
            d_roll += 0.00875;
        }
        if input.keys_held.contains(&VirtualKeyCode::C) {
            d_roll -= 0.00875;
        }
        self.camera.update_orientation(d_pitch, d_yaw, d_roll);
        self.camera
            .update_position(&input.keys_held, 5.0 * input.seconds);
        // */
    }
}

/// Acts like a normal "FPS" camera, capped at +/- 89 degrees, no roll.
#[derive(Debug, Clone, Copy)]
pub struct EulerFPSCamera {
    /// Camera position, free free to directly update at any time.
    pub position: glm::TVec3<f32>,
    pitch_deg: f32,
    yaw_deg: f32,
}
impl EulerFPSCamera {
    const UP: [f32; 3] = [0.0, 1.0, 0.0];

    fn make_front(&self) -> glm::TVec3<f32> {
        let pitch_rad = f32::to_radians(self.pitch_deg);
        let yaw_rad = f32::to_radians(self.yaw_deg);
        glm::make_vec3(&[
            yaw_rad.sin() * pitch_rad.cos(),
            pitch_rad.sin(),
            yaw_rad.cos() * pitch_rad.cos(),
        ])
    }

    /// Adjusts the camera's orientation.
    ///
    /// Input deltas should be in _degrees_, pitch is capped at +/- 89 degrees.
    pub fn update_orientation(&mut self, d_pitch_deg: f32, d_yaw_deg: f32) {
        self.pitch_deg = (self.pitch_deg + d_pitch_deg).max(-89.0).min(89.0);
        self.yaw_deg = (self.yaw_deg + d_yaw_deg) % 360.0;
    }

    /// Updates the position using WASDQE controls.
    ///
    /// The "forward" vector is relative to the current orientation.
    pub fn update_position(&mut self, keys: &HashSet<VirtualKeyCode>, distance: f32) {
        let up = glm::make_vec3(&Self::UP);
        let forward = self.make_front();
        let cross_normalized = glm::cross::<f32, glm::U3>(&forward, &up).normalize();
        let mut move_vector = keys
            .iter()
            .fold(glm::make_vec3(&[0.0, 0.0, 0.0]), |vec, key| match *key {
                VirtualKeyCode::W => vec + forward,
                VirtualKeyCode::S => vec - forward,
                VirtualKeyCode::A => vec + cross_normalized,
                VirtualKeyCode::D => vec - cross_normalized,
                VirtualKeyCode::E => vec + up,
                VirtualKeyCode::Q => vec - up,
                _ => vec,
            });
        if move_vector != glm::zero() {
            move_vector = move_vector.normalize();
            self.position += move_vector * distance;
        }
    }

    /// Generates the current view matrix for this camera.
    pub fn make_view_matrix(&self) -> glm::TMat4<f32> {
        glm::look_at_lh(
            &self.position,
            &(self.position + self.make_front()),
            &glm::make_vec3(&Self::UP),
        )
    }

    /// Makes a new camera at the position specified and Pitch/Yaw of `0.0`.
    pub const fn at_position(position: glm::TVec3<f32>) -> Self {
        Self {
            position,
            pitch_deg: 0.0,
            yaw_deg: 0.0,
        }
    }
}

/// Acts like a space flight camera.
///
/// Neat, but the fact that the user can disorient themselves means that it
/// might be too much power for the common use.
#[derive(Debug, Clone, Copy)]
pub struct QuaternionFreeCamera {
    /// Camera position, free free to update directly at any time.
    pub position: glm::TVec3<f32>,
    quat: glm::Qua<f32>,
}
impl QuaternionFreeCamera {
    /// Updates the orientation of the camera.
    ///
    /// Inputs should be in double radians, and also limited to being less than 10
    /// degrees at a time to keep approximation error minimal.
    pub fn update_orientation(&mut self, d_pitch_2rad: f32, d_yaw_2rad: f32, d_roll_2rad: f32) {
        // This gives a non-unit quaternion! That's okay because of the normalization step.
        let delta_quat = glm::quat(d_pitch_2rad, d_yaw_2rad, d_roll_2rad, 1.0);
        self.quat = glm::quat_normalize(&(self.quat * delta_quat));
    }

    /// Updates the position of the camera with WASDQE controls.
    ///
    /// All motion is relative to the current orientation.
    pub fn update_position(&mut self, keys: &HashSet<VirtualKeyCode>, distance: f32) {
        let up = glm::make_vec3(&[0.0, 1.0, 0.0]);
        let forward = glm::make_vec3(&[0.0, 0.0, 1.0]);
        let cross_normalized = glm::cross::<f32, glm::U3>(&forward, &up).normalize();
        let mut move_vector = keys
            .iter()
            .fold(glm::make_vec3(&[0.0, 0.0, 0.0]), |vec, key| match *key {
                VirtualKeyCode::W => vec + forward,
                VirtualKeyCode::S => vec - forward,
                VirtualKeyCode::A => vec + cross_normalized,
                VirtualKeyCode::D => vec - cross_normalized,
                VirtualKeyCode::E => vec + up,
                VirtualKeyCode::Q => vec - up,
                _ => vec,
            });
        if move_vector != glm::zero() {
            move_vector = move_vector.normalize();
            let rotated_move_vector = glm::quat_rotate_vec3(&self.quat, &move_vector);
            self.position += rotated_move_vector * distance;
        }
    }

    /// Generates the current view matrix for this camera.
    pub fn make_view_matrix(&self) -> glm::TMat4<f32> {
        let rotation = glm::quat_to_mat4(&self.quat);
        let translation = glm::translation(&self.position);
        glm::inverse(&(translation * rotation))
    }

    /// Makes a new camera at the position specified and an identity orientation.
    pub fn at_position(position: glm::TVec3<f32>) -> Self {
        Self {
            position,
            quat: glm::quat_identity(),
        }
    }
}

fn do_the_render(hal_state: &mut HalState, local_state: &LocalState) -> Result<(), &'static str> {
    let projection = if local_state.is_orthographic {
        local_state.orthographic_projection
    } else {
        local_state.perspective_projection
    };
    let view_projection = projection * local_state.camera.make_view_matrix();
    hal_state.draw_cubes_frame(&view_projection, &local_state.cubes)
}

fn main() {
    simple_logger::init().unwrap();

    let mut winit_state = WinitState::default();

    let mut hal_state = match HalState::new(&winit_state.window) {
        Ok(state) => state,
        Err(e) => panic!(e),
    };
    let mut local_state = {
        let (frame_width, frame_height) = winit_state
            .window
            .get_inner_size()
            .map(|logical| logical.into())
            .unwrap_or((0.0, 0.0));
        LocalState {
            frame_width,
            frame_height,
            cubes: vec![
                glm::identity(),
                glm::translate(&glm::identity(), &glm::make_vec3(&[1.5, 0.1, 0.0])),
                glm::translate(&glm::identity(), &glm::make_vec3(&[-3.0, 2.0, 3.0])),
                glm::translate(&glm::identity(), &glm::make_vec3(&[0.5, -4.0, 4.0])),
                glm::translate(&glm::identity(), &glm::make_vec3(&[-3.4, -2.3, 1.0])),
                glm::translate(&glm::identity(), &glm::make_vec3(&[-2.8, -0.7, 5.0])),
            ],
            spare_time: 0.0,
            camera: QuaternionFreeCamera::at_position(glm::make_vec3(&[0.0, 0.0, -5.0])),
            perspective_projection: {
                let mut temp =
                    glm::perspective_lh_zo(800.0 / 600.0, f32::to_radians(50.0), 0.1, 100.0);
                temp[(1, 1)] *= -1.0;
                temp
            },
            orthographic_projection: {
                let mut temp = glm::ortho_lh_zo(-5.0, 5.0, -5.0, 5.0, 0.1, 100.0);
                temp[(1, 1)] *= -1.0;
                temp
            },
            is_orthographic: false,
        }
    };
    let mut last_timestamp = Instant::now();

    loop {
        let inputs = UserInput::poll_events_loop(&mut winit_state, &mut last_timestamp);
        if inputs.end_requested {
            break;
        }
        if inputs.new_frame_size.is_some() {
            debug!("Window changed size, restarting HalState...");
            drop(hal_state);
            hal_state = match HalState::new(&winit_state.window) {
                Ok(state) => state,
                Err(e) => panic!(e),
            };
        }
        local_state.update_from_input(inputs);
        if let Err(e) = do_the_render(&mut hal_state, &local_state) {
            error!("Rendering Error: {:?}", e);
            debug!("Auto-restarting HalState...");
            drop(hal_state);
            hal_state = match HalState::new(&winit_state.window) {
                Ok(state) => state,
                Err(e) => panic!(e),
            };
        }
    }
}
