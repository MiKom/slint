// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-1.1 OR LicenseRef-Slint-commercial

use std::cell::Cell;
use std::rc::{Rc, Weak};

use crate::display::{Presenter, RenderingRotation};
use i_slint_core::api::PhysicalSize as PhysicalWindowSize;
use i_slint_core::item_rendering::ItemRenderer;
use i_slint_core::platform::PlatformError;
use i_slint_renderer_skia::SkiaRendererExt;

pub struct SkiaRendererAdapter {
    renderer: i_slint_renderer_skia::SkiaRenderer,
    presenter: Rc<dyn crate::display::Presenter>,
    size: PhysicalWindowSize,
}

impl SkiaRendererAdapter {
    #[cfg(feature = "renderer-skia-vulkan")]
    pub fn new_vulkan(
        _device_opener: &crate::DeviceOpener,
    ) -> Result<Box<dyn crate::fullscreenwindowadapter::FullscreenRenderer>, PlatformError> {
        // TODO: figure out how to associate vulkan with an existing drm fd.
        let display = crate::display::vulkandisplay::create_vulkan_display()?;

        let skia_vk_surface = i_slint_renderer_skia::vulkan_surface::VulkanSurface::from_surface(
            display.physical_device,
            display.queue_family_index,
            display.surface,
            display.size,
        )?;

        let renderer = Box::new(Self {
            renderer: i_slint_renderer_skia::SkiaRenderer::new_with_surface(Box::new(
                skia_vk_surface,
            )),
            // TODO: For vulkan we don't have a page flip event handling mechanism yet, so drive it with a timer.
            presenter: TimerBasedAnimationDriver::new(),
            size: display.size,
        });

        eprintln!("Using Skia Vulkan renderer");

        Ok(renderer)
    }

    #[cfg(feature = "renderer-skia-opengl")]
    pub fn new_opengl(
        device_opener: &crate::DeviceOpener,
    ) -> Result<Box<dyn crate::fullscreenwindowadapter::FullscreenRenderer>, PlatformError> {
        let display = crate::display::egldisplay::create_egl_display(device_opener)?;

        use i_slint_renderer_skia::Surface;
        use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
        let skia_gl_surface = i_slint_renderer_skia::opengl_surface::OpenGLSurface::new(
            display.window_handle().unwrap(),
            display.display_handle().unwrap(),
            display.size,
        )?;

        let size = display.size;

        let renderer = Box::new(Self {
            renderer: i_slint_renderer_skia::SkiaRenderer::new_with_surface(Box::new(
                skia_gl_surface,
            )),
            presenter: Rc::new(display),
            size,
        });

        eprintln!("Using Skia OpenGL renderer");

        Ok(renderer)
    }

    pub fn new_try_vulkan_then_opengl(
        device_opener: &crate::DeviceOpener,
    ) -> Result<Box<dyn crate::fullscreenwindowadapter::FullscreenRenderer>, PlatformError> {
        #[allow(unused_assignments)]
        let mut result = Err(format!("No skia renderer available").into());

        #[cfg(feature = "renderer-skia-vulkan")]
        {
            result = Self::new_vulkan(device_opener);
        }

        #[cfg(feature = "renderer-skia-opengl")]
        if result.is_err() {
            result = Self::new_opengl(device_opener);
        }

        result
    }
}

impl crate::fullscreenwindowadapter::FullscreenRenderer for SkiaRendererAdapter {
    fn as_core_renderer(&self) -> &dyn i_slint_core::renderer::Renderer {
        &self.renderer
    }

    fn is_ready_to_present(&self) -> bool {
        self.presenter.is_ready_to_present()
    }

    fn render_and_present(
        &self,
        rotation: RenderingRotation,
        draw_mouse_cursor_callback: &dyn Fn(&mut dyn ItemRenderer),
        ready_for_next_animation_frame: Box<dyn FnOnce()>,
    ) -> Result<(), PlatformError> {
        self.renderer.render_transformed_with_post_callback(
            rotation.degrees(),
            rotation.translation_after_rotation(self.size),
            self.size,
            Some(&|item_renderer| {
                draw_mouse_cursor_callback(item_renderer);
            }),
        )?;
        self.presenter.present_with_next_frame_callback(ready_for_next_animation_frame)?;
        Ok(())
    }
    fn size(&self) -> i_slint_core::api::PhysicalSize {
        self.size
    }

    fn register_page_flip_handler(
        &self,
        event_loop_handle: crate::calloop_backend::EventLoopHandle,
    ) -> Result<(), PlatformError> {
        self.presenter.clone().register_page_flip_handler(event_loop_handle)
    }
}

struct TimerBasedAnimationDriver {
    timer: i_slint_core::timers::Timer,
    next_animation_frame_callback: Cell<Option<Box<dyn FnOnce()>>>,
}

impl TimerBasedAnimationDriver {
    fn new() -> Rc<Self> {
        Rc::new_cyclic(|self_weak: &Weak<Self>| {
            let self_weak = self_weak.clone();
            let timer = i_slint_core::timers::Timer::default();
            timer.start(
                i_slint_core::timers::TimerMode::Repeated,
                std::time::Duration::from_millis(16),
                move || {
                    let Some(this) = self_weak.upgrade() else { return };
                    // Stop the timer and let the callback decide if we need to continue. It will set
                    // `needs_redraw` to true of animations should continue, render() will be called,
                    // present_with_next_frame_callback() will be called and then the timer restarted.
                    this.timer.stop();
                    if let Some(next_animation_frame_callback) =
                        this.next_animation_frame_callback.take()
                    {
                        next_animation_frame_callback();
                    }
                },
            );
            // Activate it only when we present a frame.
            timer.stop();

            Self { timer, next_animation_frame_callback: Default::default() }
        })
    }
}

impl Presenter for TimerBasedAnimationDriver {
    fn is_ready_to_present(&self) -> bool {
        true
    }

    fn register_page_flip_handler(
        self: Rc<Self>,
        _event_loop_handle: crate::calloop_backend::EventLoopHandle,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    fn present_with_next_frame_callback(
        &self,
        ready_for_next_animation_frame: Box<dyn FnOnce()>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.next_animation_frame_callback.set(Some(ready_for_next_animation_frame));
        self.timer.restart();
        Ok(())
    }
}
