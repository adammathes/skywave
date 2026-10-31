//! Per-frame recording: planning, culling, packing and the passes of a frame.

use super::*;

impl Renderer {
    /// Renders `scene` into `view`, which must have the surface format given to `new` and
    /// the size given to `new`/`resize`.
    ///
    /// Allocates nothing on the heap or the GPU unless the instance count exceeds every
    /// earlier frame's (and [`Renderer::reserve_instances`]); in debug builds that is an
    /// assertion failure after [`WARMUP_FRAMES`]. A changed [`SceneView::quality`] may
    /// rebuild targets first (counted in [`RenderStats::rebuilds`]).
    pub fn render(&mut self, view: &wgpu::TextureView, scene: &SceneView) {
        let hud_dropped = self.build_hud(scene);
        self.render_built(view, scene, hud_dropped);
    }

    /// [`Renderer::render`] with `scene`'s HUD quads already in `hud_staging`
    /// ([`Renderer::build_hud`], which dropped `hud_dropped`).
    pub(crate) fn render_built(
        &mut self,
        view: &wgpu::TextureView,
        scene: &SceneView,
        hud_dropped: u32,
    ) {
        self.set_quality(scene.quality);
        if let Some(t) = &mut self.timer {
            t.begin_frame(&self.device);
        }
        let view_m = Mat4::from_cols_array_2d(&scene.camera.view);
        let proj = Mat4::from_cols_array_2d(&scene.camera.proj);
        let sky = SkyModel::new(&scene.sun, &scene.sky);
        let n = self.quality.cascades as usize;
        let far = n - 1;
        // Once the sun has stopped lighting, no cascade is fitted, culled or drawn, and the
        // forward pass skips the shadow test.
        let sun_lit = sky.sun_lit();
        let mut fits = if sun_lit {
            self.fit_shadows(view_m, proj, sky.sun_dir)
        } else {
            [NO_FIT; CASCADES]
        };
        // A cascade whose kept map still covers its slice is kept when its casters have
        // not changed and the sun has not moved enough to shift a shadow by half a texel;
        // the farthest, between its frames ([`RenderQuality::far_cascade_interval`]), even
        // when they have.
        let interval = u64::from(self.quality.far_cascade_interval);
        // The first world frame after kept or skipped ones holds nothing unchecked: the
        // frames between were not drawn, so nothing would catch up a held cascade.
        let resumed = self.stats.last_work != FrameWork::World;
        let mut keepable = [false; CASCADES];
        let mut cull = [false; CASCADES];
        self.shadows_held = false;
        if sun_lit {
            for k in 0..n {
                let held = k == far
                    && interval > 1
                    && !resumed
                    && !self.stats.frames.is_multiple_of(interval);
                match self.kept_map(k, &fits[k], sky.sun_dir) {
                    Some((kept, _)) if held => {
                        fits[k] = kept.fit;
                        self.shadows_held = true;
                    }
                    Some((kept, true)) => {
                        fits[k] = kept.fit;
                        keepable[k] = true;
                        cull[k] = true;
                    }
                    _ => cull[k] = true,
                }
            }
        } else {
            // Nothing kept: the first sunlit frame draws every cascade.
            self.kept = [None; CASCADES];
        }
        if n == 2 {
            fits[2] = fits[1];
        }
        let (uniform, cascades) = self.frame_uniforms_with(scene, &sky, &fits);
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));
        self.queue
            .write_buffer(&self.cascade_buffer, 0, bytemuck::bytes_of(&cascades));

        if scene.instances.len() > self.instance_capacity {
            debug_assert!(
                self.stats.frames < WARMUP_FRAMES,
                "instance buffer grew after warmup ({} instances): call Renderer::reserve_instances while loading",
                scene.instances.len()
            );
            self.reserve_instances(scene.instances.len());
            self.stats.resources_created += 1;
        }
        let lights = scene.lights.len().min(MAX_LIGHTS);
        let hashes = self.cull(scene, lights, proj * view_m, &fits, cull);
        let drawn: [bool; CASCADES] = std::array::from_fn(|k| {
            let same = self.kept[k].is_some_and(|kept| kept.hash == hashes[k]);
            cull[k] && !(keepable[k] && same)
        });
        for k in 0..n {
            if drawn[k] {
                self.kept[k] = Some(KeptCascade {
                    fit: fits[k],
                    sun: sky.sun_dir,
                    hash: hashes[k],
                });
            }
        }
        let (ranges, late_start, glass_start) = self.pack(scene, drawn);
        // Which lights any instance uses, for the stats.
        let mut used = [0u64; MAX_LIGHTS / 64];
        for inst in scene.instances {
            for &i in inst.lights.indices() {
                if usize::from(i) < lights {
                    used[usize::from(i) / 64] |= 1 << (i % 64);
                }
            }
        }
        self.corona_staging.clear();
        if self.quality.coronas && self.corona_probe.poll() == Probe::Passed {
            self.push_coronas(scene);
        }
        let coronas = self.corona_staging.len() as u32;
        if coronas > 0 {
            self.queue.write_buffer(
                &self.corona_buffer,
                0,
                bytemuck::cast_slice(&self.corona_staging),
            );
        }
        if !self.instance_staging.is_empty() {
            self.queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&self.instance_staging),
            );
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        // Passes that wrote their GPU timestamps. The timer is taken for the frame so
        // draws can count their fragments into it.
        let mut timed = 0u8;
        let mut timer = self.timer.take();
        fn ts(
            timer: &Option<GpuTimer>,
            pass: usize,
            begin: bool,
            end: bool,
        ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
            timer.as_ref().and_then(|t| t.writes(pass, begin, end))
        }
        let occlusion = timer.as_ref().and_then(|t| t.occlusion_set()).cloned();
        let mut shadow_draws = 0;
        let mut shadow_instances = [0u32; CASCADES];
        let mut shadow_triangles = 0u64;
        let mut buffer_binds = 0;
        // Every cascade kept: no shadow pass at all (a tiled GPU would still load and store
        // the whole atlas for an empty one).
        if drawn.iter().any(|&d| d) {
            // Every cascade drawn: clear the atlas. Some kept: clear only the drawn ones.
            let all = (0..n).all(|k| drawn[k]);
            let load = if all {
                wgpu::LoadOp::Clear(1.0)
            } else {
                wgpu::LoadOp::Load
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: ts(&timer, timing::PASS_SHADOW, true, true),
                occlusion_query_set: occlusion.as_ref(),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.frame_group, &[]);
            let res = self.cascade_size;
            for (k, group) in self.cascade_groups.iter().enumerate() {
                if !drawn[k] {
                    continue;
                }
                let x = k as u32 * res;
                pass.set_viewport(x as f32, 0.0, res as f32, res as f32, 0.0, 1.0);
                // GL does not clip to the viewport; the scissor keeps cascades apart.
                pass.set_scissor_rect(x, 0, res, res);
                pass.set_bind_group(1, group, &[]);
                if let Some(t) = &mut timer {
                    t.part(&mut pass, k, true);
                }
                if !all {
                    pass.set_pipeline(&self.shadow_clear_pipeline);
                    pass.draw(0..3, 0..1);
                    shadow_draws += 1;
                }
                pass.set_pipeline(&self.shadow_pipeline);
                let range = ranges[1 + k].clone();
                shadow_instances[k] = range.len() as u32;
                if let Some(t) = &mut timer {
                    t.begin_count(&mut pass, Some(k));
                }
                let (draws, tris, b) = self.draw_instances(&mut pass, scene.instances, range, None);
                if let Some(t) = &mut timer {
                    if t.occlusion_set().is_some() {
                        pass.end_occlusion_query();
                    }
                    t.part(&mut pass, k, false);
                }
                shadow_draws += draws;
                shadow_triangles += tris;
                buffer_binds += b;
            }
            drop(pass);
            timed |= 1 << timing::PASS_SHADOW;
        }
        let mut forward_draws = 1;
        let mut triangles;
        {
            let color = self.targets.hdr.as_ref().unwrap_or(view);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("forward"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        // Kept for the corona pass's occlusion test.
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: ts(&timer, timing::PASS_FORWARD, true, true),
                occlusion_query_set: occlusion.as_ref(),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_bind_group(1, &self.forward_group, &[]);
            // The depth pre-pass: the meshes' depth (not the late backdrop's, which they
            // mostly hide), so the meshes after it shade each pixel about once.
            if self.quality.depth_prepass {
                if let Some(t) = &mut timer {
                    t.part(&mut pass, timing::PART_DEPTH, true);
                }
                pass.set_pipeline(&self.depth_pipeline);
                let range = ranges[0].start..late_start;
                let (d, _, b) = self.draw_instances(&mut pass, scene.instances, range, None);
                forward_draws += d;
                buffer_binds += b;
                if let Some(t) = &mut timer {
                    t.part(&mut pass, timing::PART_DEPTH, false);
                }
                pass.set_pipeline(&self.mesh_after_depth_pipeline);
            } else {
                pass.set_pipeline(&self.mesh_pipeline);
            }
            let parts = [
                (timing::PART_MESHES, ranges[0].start..late_start),
                (timing::PART_LATE, late_start..glass_start),
            ];
            let mut tris = 0;
            for (part, range) in parts {
                if let Some(t) = &mut timer {
                    t.part(&mut pass, part, true);
                }
                let (d, n, b) =
                    self.draw_instances(&mut pass, scene.instances, range, timer.as_mut());
                if let Some(t) = &mut timer {
                    t.part(&mut pass, part, false);
                }
                forward_draws += d;
                tris += n;
                buffer_binds += b;
            }
            triangles = tris;
            // The sky last, at the far plane: only the pixels no mesh covered shade it.
            if let Some(t) = &mut timer {
                t.part(&mut pass, timing::PART_SKY, true);
                t.begin_count(&mut pass, None);
            }
            pass.set_pipeline(&self.sky_pipeline);
            pass.draw(0..3, 0..1);
            if let Some(t) = &mut timer {
                if t.occlusion_set().is_some() {
                    pass.end_occlusion_query();
                }
                t.part(&mut pass, timing::PART_SKY, false);
            }
            // Glass over everything behind it, sky included.
            if glass_start < ranges[0].end {
                pass.set_pipeline(&self.glass_pipeline);
                let (d, n, b) = self.draw_instances(
                    &mut pass,
                    scene.instances,
                    glass_start..ranges[0].end,
                    None,
                );
                forward_draws += d;
                triangles += n;
                buffer_binds += b;
            }
        }
        timed |= 1 << timing::PASS_FORWARD;
        if coronas > 0 {
            let color = self.targets.hdr.as_ref().unwrap_or(view);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("coronas"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                timestamp_writes: ts(&timer, timing::PASS_CORONA, true, true),
                ..Default::default()
            });
            pass.set_pipeline(&self.corona_pipeline);
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_bind_group(1, &self.targets.corona_group, &[]);
            pass.set_vertex_buffer(0, self.corona_buffer.slice(..));
            pass.draw(0..4, 0..coronas);
            forward_draws += 1;
            timed |= 1 << timing::PASS_CORONA;
        }
        let mut bloom_draws = 0u32;
        let mut bloom_levels = 0;
        if let (Some(post), Some(targets)) = (&self.post, &self.targets.post) {
            if scene.sky.bloom_strength > 0.0 {
                let levels = self.quality.bloom_levels as usize;
                let last = 2 * levels as u32 - 2;
                // Down the chain: the prefilter reads the HDR target, each later level the
                // one before it.
                for (k, level) in targets.bloom.iter().take(levels).enumerate() {
                    let (pipeline, group) = match k {
                        0 => (&post.prefilter, &targets.hdr_group),
                        _ => (&post.down, &targets.level_groups[k - 1]),
                    };
                    let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
                    fullscreen_pass(
                        &mut encoder,
                        level,
                        clear,
                        pipeline,
                        [&self.frame_group, group],
                        ts(&timer, timing::PASS_BLOOM, k == 0, bloom_draws == last),
                    );
                    bloom_draws += 1;
                }
                // Back up: each level adds a tent-filtered copy of the (already summed)
                // level below it.
                for k in (0..levels - 1).rev() {
                    let groups = [&self.frame_group, &targets.level_groups[k + 1]];
                    let level = &targets.bloom[k];
                    let writes = ts(&timer, timing::PASS_BLOOM, false, bloom_draws == last);
                    fullscreen_pass(
                        &mut encoder,
                        level,
                        wgpu::LoadOp::Load,
                        &post.up,
                        groups,
                        writes,
                    );
                    bloom_draws += 1;
                }
                bloom_levels = levels as u32;
                timed |= 1 << timing::PASS_BLOOM;
            }
            let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
            let groups = [&self.frame_group, &targets.tonemap_group];
            let writes = ts(&timer, timing::PASS_TONEMAP, true, true);
            fullscreen_pass(&mut encoder, view, clear, &post.tonemap, groups, writes);
            forward_draws += 1;
            timed |= 1 << timing::PASS_TONEMAP;
        }
        let hud_quads = self.draw_hud(&mut encoder, view, ts(&timer, timing::PASS_HUD, true, true));
        if hud_quads > 0 {
            forward_draws += 1;
            timed |= 1 << timing::PASS_HUD;
        }
        if let Some(t) = &mut timer {
            for p in 0..GPU_PASSES {
                if timed & (1 << p) != 0 {
                    t.mark(p);
                }
            }
            t.resolve(&mut encoder);
        }
        self.queue.submit([encoder.finish()]);
        if let Some(t) = &mut timer {
            t.after_submit(self.stats.frames + 1);
        }
        self.timer = timer;
        self.shadow_valid = true;
        let coronas_on = self.coronas_on(scene.quality);
        self.kept_world.keep(scene, coronas_on);
        self.note_hud_drawn();
        self.stats.frames += 1;
        self.stats.last_work = FrameWork::World;
        self.stats.shadow_draw_calls = shadow_draws;
        self.stats.bloom_draw_calls = bloom_draws;
        self.stats.buffer_binds = buffer_binds;
        self.stats.draw_calls = shadow_draws + forward_draws + bloom_draws;
        self.stats.lights_submitted = lights as u32;
        self.stats.lights_used = used.iter().map(|w| w.count_ones()).sum();
        self.stats.coronas = coronas;
        self.stats.instances = scene.instances.len() as u32;
        self.stats.instances_drawn = ranges[0].len() as u32;
        let (mut full, mut slots) = (0, 0);
        for &i in &self.order[ranges[0].clone()] {
            let n = scene.instances[i as usize].lights.len() as u32;
            full += u32::from(n == LIGHTS_PER_DRAW as u32);
            slots += n;
        }
        self.stats.full_light_sets = full;
        self.stats.light_slots = slots;
        self.stats.shadow_instances = shadow_instances;
        self.stats.triangles = triangles;
        self.stats.shadow_triangles = shadow_triangles;
        self.stats.shadow_size = self.cascade_size;
        self.stats.cascades = n as u32;
        self.stats.cascades_drawn = drawn.iter().filter(|&&d| d).count() as u32;
        self.stats.surface_size = self.size;
        self.stats.render_size = self.targets.size;
        self.stats.bloom_levels = bloom_levels;
        self.stats.gpu_timing = self.timer.is_some();
        if let Some(t) = &self.timer {
            self.stats.gpu_parts = t.inside;
            self.stats.gpu_us = t.last_us;
            self.stats.gpu_part_us = t.last_part_us;
            self.stats.gpu_median_us = t.median_us;
            self.stats.gpu_median_part_us = t.median_part_us;
            self.stats.gpu_counts = t.counts;
            self.stats.gpu_frame = t.frame;
        }
        self.stats.hud_quads = hud_quads;
        self.stats.hud_quads_dropped = hud_dropped;
    }

    /// What drawing `scene` takes: [`FrameWork::Skip`] when the last frame drew the same
    /// world (camera, sun, sky, quality, every instance and light compared exactly) and the
    /// same HUD, [`FrameWork::Ui`] when only the HUD differs and the world is kept in an
    /// HDR target, else [`FrameWork::World`]. Costs a compare of the instances (tens of
    /// microseconds for a few thousand) and building the HUD's quads.
    pub fn plan(&mut self, scene: &SceneView) -> FrameWork {
        self.plan_built(scene).0
    }

    /// [`Renderer::plan`], leaving `scene`'s HUD quads in `hud_staging` for the frame to
    /// draw; also returns how many quads were dropped.
    pub(crate) fn plan_built(&mut self, scene: &SceneView) -> (FrameWork, u32) {
        let dropped = self.build_hud(scene);
        let coronas = self.coronas_on(scene.quality);
        let work = if self.shadows_held || !self.kept_world.same(scene, coronas) {
            FrameWork::World
        } else if self.hud_valid && self.hud_staging == self.hud_drawn {
            FrameWork::Skip
        } else if self.post.is_some() && self.targets.post.is_some() {
            FrameWork::Ui
        } else {
            FrameWork::World
        };
        (work, dropped)
    }

    /// Draws `scene` with no more work than [`Renderer::plan`] says it takes, into the
    /// target `acquire` hands out (the swapchain texture and its view; called only when
    /// there is something to draw). Returns the work done and the frame to present, if
    /// one was acquired.
    pub fn draw<F>(
        &mut self,
        scene: &SceneView,
        acquire: impl FnOnce() -> Option<(F, wgpu::TextureView)>,
    ) -> (FrameWork, Option<F>) {
        let (work, hud_dropped) = self.plan_built(scene);
        if work == FrameWork::Skip {
            self.stats.skipped_frames += 1;
            self.stats.last_work = work;
            return (work, None);
        }
        let Some((frame, view)) = acquire() else {
            return (work, None);
        };
        match work {
            FrameWork::Ui => self.render_ui_built(&view, scene, hud_dropped),
            _ => self.render_built(&view, scene, hud_dropped),
        }
        (work, Some(frame))
    }

    /// Tonemaps the world kept from the last world frame into `view` again and draws
    /// `scene`'s HUD over it ([`FrameWork::Ui`]). Draws the whole frame instead when no
    /// world is kept (none drawn yet, or no HDR target to keep it in).
    pub fn render_ui(&mut self, view: &wgpu::TextureView, scene: &SceneView) {
        let hud_dropped = self.build_hud(scene);
        self.render_ui_built(view, scene, hud_dropped);
    }

    /// [`Renderer::render_ui`] with `scene`'s HUD quads already in `hud_staging`.
    pub(crate) fn render_ui_built(
        &mut self,
        view: &wgpu::TextureView,
        scene: &SceneView,
        hud_dropped: u32,
    ) {
        let (Some(post), Some(targets), Some(_)) =
            (&self.post, &self.targets.post, &self.kept_world.head)
        else {
            return self.render_built(view, scene, hud_dropped);
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ui") });
        let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
        let groups = [&self.frame_group, &targets.tonemap_group];
        fullscreen_pass(&mut encoder, view, clear, &post.tonemap, groups, None);
        let quads = self.draw_hud(&mut encoder, view, None);
        self.queue.submit([encoder.finish()]);
        self.note_hud_drawn();
        self.stats.ui_frames += 1;
        self.stats.last_work = FrameWork::Ui;
        self.stats.hud_quads = quads;
        self.stats.hud_quads_dropped = hud_dropped;
    }

    /// Builds `scene`'s HUD quads into `hud_staging`; returns how many were dropped.
    pub(crate) fn build_hud(&mut self, scene: &SceneView) -> u32 {
        self.hud_staging.clear();
        hud::build_quads(
            &scene.hud,
            self.size,
            self.map_size,
            self.stills_size,
            &mut self.hud_staging,
            MAX_HUD_QUADS,
        )
    }

    /// Uploads the quads in `hud_staging` and draws them over `view`, if there are any;
    /// returns how many.
    pub(crate) fn draw_hud(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) -> u32 {
        let quads = self.hud_staging.len() as u32;
        if quads > 0 {
            self.queue
                .write_buffer(&self.hud_buffer, 0, bytemuck::cast_slice(&self.hud_staging));
            let groups = [&self.frame_group, &self.hud_group];
            let (pipeline, buffer) = (&self.hud_pipeline, &self.hud_buffer);
            hud_pass(
                encoder,
                view,
                pipeline,
                groups,
                buffer,
                quads,
                timestamp_writes,
            );
        }
        quads
    }

    /// The screen no longer shows the last frame (the window was exposed, or nothing was
    /// drawn into it for a while): the next frame draws at least the tonemap and the HUD.
    pub fn screen_lost(&mut self) {
        self.hud_valid = false;
    }

    /// The corona pass runs at `quality` (and its start-up check has passed).
    pub(crate) fn coronas_on(&mut self, quality: RenderQuality) -> bool {
        quality.coronas && self.corona_probe.poll() == Probe::Passed
    }

    /// The quads just drawn (in `hud_staging`) are the HUD on screen.
    pub(crate) fn note_hud_drawn(&mut self) {
        self.hud_drawn.clear();
        self.hud_drawn.extend_from_slice(&self.hud_staging);
        self.hud_valid = true;
    }

    /// The cascades fitted to the camera: three over the cascade splits, or two over
    /// [`TWO_CASCADE_SPLITS`] (the third slot a copy of the second). A farthest cascade
    /// that may be kept over frames gets [`FAR_CASCADE_PAD`] of extra room (its `radius`
    /// stays the slice's).
    pub(crate) fn fit_shadows(
        &self,
        view: Mat4,
        proj: Mat4,
        to_sun: Vec3,
    ) -> [CascadeFit; CASCADES] {
        let n = self.quality.cascades as usize;
        let splits = if n == 2 {
            let [a, b] = TWO_CASCADE_SPLITS;
            [a, b, b]
        } else {
            self.cascade_splits
        };
        let res = self.cascade_size;
        let mut fits = fit_cascades(
            view,
            proj,
            to_sun,
            splits,
            CASCADE_BLEND,
            res,
            CASTER_MARGIN,
        );
        if self.quality.far_cascade_interval > 1 {
            let f = &mut fits[n - 1];
            let padded = f.radius * (1.0 + FAR_CASCADE_PAD);
            f.view_proj = sun_view_proj(to_sun, f.center, padded, CASTER_MARGIN, res);
            f.texel = 2.0 * padded / res as f32;
        }
        if n == 2 {
            fits[2] = fits[1];
        }
        fits
    }

    /// Cascade `k`'s map from an earlier frame, when it can stand in for this frame's
    /// (`fit`) for a frame or two: the atlas still holds it, it covers this frame's slice
    /// sphere (to half a texel; the farthest cascade is fitted wider when it may be kept,
    /// see [`FAR_CASCADE_PAD`]) and the sun has moved less than 0.08°. The flag says it may
    /// stand in for good: the sun has moved too little to shift the shadow of a
    /// [`TALL_CASTER`] by half a texel.
    pub(crate) fn kept_map(
        &self,
        k: usize,
        fit: &CascadeFit,
        to_sun: Vec3,
    ) -> Option<(KeptCascade, bool)> {
        let kept = self.kept[k].filter(|_| self.shadow_valid)?;
        let texel = kept.fit.texel;
        let half = 0.5 * texel * self.cascade_size as f32;
        let covered = kept.fit.center.distance(fit.center) + fit.radius <= half + 0.5 * texel;
        // The angle the sun moved (its sine, exact for small angles); a shadow's tip moves
        // by the caster's height × the angle / sin² of the sun's elevation.
        let moved = kept.sun.cross(to_sun).length();
        let still = kept.sun.dot(to_sun) >= 1.0 - 1e-6;
        let unchanged = moved * TALL_CASTER <= 0.5 * texel * to_sun.y * to_sun.y;
        (covered && still).then_some((kept, unchanged))
    }

    /// [`Renderer::cull`] then [`Renderer::pack`] every culled cascade.
    #[cfg(test)]
    pub(crate) fn cull_and_pack(
        &mut self,
        scene: &SceneView,
        lights: usize,
        view_proj: Mat4,
        fits: &[CascadeFit; CASCADES],
        drawn: [bool; CASCADES],
    ) -> ([std::ops::Range<usize>; PASSES], usize) {
        self.cull(scene, lights, view_proj, fits, drawn);
        let (ranges, late, _) = self.pack(scene, drawn);
        (ranges, late)
    }

    /// Culls every instance for the forward pass (`view_proj`) and each cascade in `cull`.
    /// Returns a hash of each cascade's casters (meshes and transforms, in order, and the
    /// chunk pool's writes), so a cascade whose casters have not changed can be kept.
    pub(crate) fn cull(
        &mut self,
        scene: &SceneView,
        lights: usize,
        view_proj: Mat4,
        fits: &[CascadeFit; CASCADES],
        cull: [bool; CASCADES],
    ) -> [u64; CASCADES] {
        let camera = Frustum::from_matrix(view_proj);
        let eye = Mat4::from_cols_array_2d(&scene.camera.view)
            .inverse()
            .w_axis
            .xyz();
        let boxes = fits.map(|f| Frustum::from_matrix(f.view_proj));
        let min_texels = self.quality.min_caster_texels;
        self.vis.clear();
        self.raws.clear();
        // Every chunk written into the pool changes every hash: a slot freed and written
        // again keeps its part ids (and chunks are drawn untransformed) but not its mesh.
        let writes = self.chunk_pool.stats().writes;
        let seed = fnv(fnv(FNV_OFFSET, writes as u32), (writes >> 32) as u32);
        let mut hashes = [seed; CASCADES];
        for inst in scene.instances {
            let mut bits = 0u8;
            if let Some(b) = self.mesh_bounds(inst.mesh) {
                let (c, h) = b.transformed(&Mat4::from_cols_array_2d(&inst.transform));
                if !self.culling || camera.intersects(c, h) {
                    bits |= 1;
                }
                if inst.casts_shadow {
                    let diagonal = 2.0 * h.length();
                    for k in 0..CASCADES {
                        let big = diagonal >= min_texels * fits[k].texel;
                        if cull[k] && big && (!self.culling || boxes[k].intersects(c, h)) {
                            bits |= 2 << k;
                            let mut hash = fnv(hashes[k], inst.mesh.0);
                            for v in inst.transform.as_flattened() {
                                hash = fnv(hash, v.to_bits());
                            }
                            hashes[k] = hash;
                        }
                    }
                }
            }
            self.vis.push(bits);
            self.raws.push(if bits != 0 {
                InstanceRaw::new(inst, lights, eye)
            } else {
                InstanceRaw::zeroed()
            });
        }
        hashes
    }

    /// Packs the culled instances into `instance_staging`: the forward pass first (its
    /// `late` instances, then its `glass`, last), then each cascade in `drawn`. Returns
    /// each pass's range of it and where the forward pass's `late` and `glass` instances
    /// start.
    pub(crate) fn pack(
        &mut self,
        scene: &SceneView,
        drawn: [bool; CASCADES],
    ) -> ([std::ops::Range<usize>; PASSES], usize, usize) {
        self.order.clear();
        self.instance_staging.clear();
        let mut ranges: [std::ops::Range<usize>; PASSES] = std::array::from_fn(|_| 0..0);
        let (mut late_start, mut glass_start) = (0, 0);
        // Which forward sweep an instance is in: the rest, late, glass.
        let sweep_of = |inst: &MeshInstance| {
            if inst.glass {
                2
            } else {
                u8::from(inst.late)
            }
        };
        for (p, range) in ranges.iter_mut().enumerate() {
            let start = self.order.len();
            if p > 0 && !drawn[p - 1] {
                *range = start..start;
                continue;
            }
            let bit = 1u8 << p;
            // The forward pass draws `late` instances after the rest, and glass last.
            let sweeps: &[Option<u8>] = if p == 0 {
                &[Some(0), Some(1), Some(2)]
            } else {
                &[None]
            };
            for &key in sweeps {
                if key == Some(1) {
                    late_start = self.order.len();
                }
                if key == Some(2) {
                    glass_start = self.order.len();
                }
                let sweep = self.order.len();
                for (i, inst) in scene.instances.iter().enumerate() {
                    if self.vis[i] & bit != 0 && key.is_none_or(|k| sweep_of(inst) == k) {
                        self.order.push(i as u32);
                    }
                }
                // Coplanar surfaces of different meshes (a car's trim on its body) resolve by
                // draw order: the first drawn wins a less-than test. After the pre-pass the
                // test is less-or-equal, where the last drawn wins, so the meshes go in
                // reverse and the same one wins.
                if key == Some(0) && self.quality.depth_prepass {
                    self.order[sweep..].reverse();
                }
                for &i in &self.order[sweep..] {
                    self.instance_staging.push(self.raws[i as usize]);
                }
            }
            *range = start..self.order.len();
        }
        (ranges, late_start, glass_start)
    }

    /// Model-space bounds of what `id` draws, or `None` when it draws nothing.
    pub(crate) fn mesh_bounds(&self, id: MeshId) -> Option<Aabb> {
        let b = match id.as_chunk_part() {
            Some(_) => self.chunk_pool.part_bounds(id)?,
            None => self.meshes.get(id.0 as usize)?.bounds,
        };
        (!b.is_empty()).then_some(b)
    }

    /// Fills `corona_staging` with the frame's coronas: one per light with a [`Corona`],
    /// then the sun and the moon, each as bright as its light against the exposure (see
    /// [`CORONA_PEAK`]); the invisible ones are left out.
    pub(crate) fn push_coronas(&mut self, scene: &SceneView) {
        let sky = &scene.sky;
        let strength = sky.corona_strength.clamp(0.0, 1.0);
        if strength <= 0.0 {
            return;
        }
        let exposure = sky.exposure.max(1e-6);
        let luma = |c: Vec3| c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        // Radiance whose brightest channel lands at the knee of `x` after exposure, in the
        // hue of `color`, or none when that is too dim to see.
        let peak = |color: Vec3, x: f32, k: f32| {
            let e = CORONA_PEAK * (x / (1.0 + x)) * k * strength;
            let max = color.max_element();
            (e >= CORONA_MIN_PEAK && max > 0.0).then(|| color * (e / (max * exposure)))
        };
        for light in &scene.lights[..scene.lights.len().min(MAX_LIGHTS)] {
            let c = light.corona;
            if c.radius <= 0.0 || c.strength <= 0.0 {
                continue;
            }
            let color = Vec3::from(light.color).max(Vec3::ZERO);
            let x = luma(color) * exposure * CORONA_GAIN;
            if let Some(rgb) = peak(color, x, c.strength.min(1.0)) {
                let [px, py, pz] = light.position;
                self.corona_staging.push(CoronaRaw {
                    pos_size: [px, py, pz, c.radius],
                    color: rgb.extend(0.0).to_array(),
                });
            }
        }
        // The sun and moon, while above the horizon.
        let sun_dir = Vec3::from(scene.sun.direction).normalize_or(Vec3::Y);
        let moon_dir = Vec3::from(sky.moon_direction).normalize_or(Vec3::NEG_Y);
        let above = |d: Vec3| smoothstep(-0.01, 0.02, d.y);
        let sun = Vec3::from(scene.sun.illuminance).max(Vec3::ZERO);
        let x = luma(sun) * exposure * SUN_CORONA_GAIN;
        if let Some(rgb) = peak(sun, x, SUN_CORONA_STRENGTH * above(sun_dir)) {
            self.corona_staging.push(CoronaRaw {
                pos_size: sun_dir.extend(-SUN_CORONA_RADIUS).to_array(),
                color: rgb.extend(0.0).to_array(),
            });
        }
        // The lit fraction of the disc follows from the angle between the sun and moon.
        let lit = 0.5 * (1.0 - sun_dir.dot(moon_dir));
        let x = sky.moon_luminance.max(0.0) * lit * exposure * MOON_CORONA_GAIN;
        let moonlight = Vec3::new(1.0, 0.96, 0.9);
        if let Some(rgb) = peak(moonlight, x, MOON_CORONA_STRENGTH * above(moon_dir)) {
            self.corona_staging.push(CoronaRaw {
                pos_size: moon_dir.extend(-MOON_CORONA_RADIUS).to_array(),
                color: rgb.extend(0.0).to_array(),
            });
        }
    }

    /// Issues one instanced draw per run of consecutive packed instances in `range` (of
    /// `order` and the instance buffer) sharing a mesh; returns the number of draws, of
    /// triangles and of buffer binds. The chunk pool's buffers are bound before its first
    /// draw and again only after a standalone mesh bound its own. With a `timer`, each
    /// draw counts its fragments.
    pub(crate) fn draw_instances(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        instances: &[MeshInstance],
        range: std::ops::Range<usize>,
        mut timer: Option<&mut GpuTimer>,
    ) -> (u32, u64, u32) {
        pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        let order = &self.order[range.clone()];
        let mesh_of = |k: usize| instances[order[k] as usize].mesh;
        let (mut draws, mut triangles, mut binds) = (0, 0u64, 0);
        let mut pool_bound = false;
        let mut start = 0;
        while start < order.len() {
            let id = mesh_of(start);
            let mut end = start + 1;
            while end < order.len() && mesh_of(end) == id {
                end += 1;
            }
            let packed = (range.start + start) as u32..(range.start + end) as u32;
            let copies = (end - start) as u64;
            let counted = timer.as_deref_mut().is_some_and(|t| {
                let run = &order[start..end];
                let lights: usize = run
                    .iter()
                    .map(|&i| instances[i as usize].lights.len())
                    .sum();
                let first = &instances[run[0] as usize];
                t.begin_draw(
                    pass,
                    DrawWeight {
                        lights: lights as f32 / run.len() as f32,
                        shadowed: first.receives_shadow,
                        late: first.late,
                    },
                )
            });
            if let Some([first, count]) = self.chunk_pool.draw_range(id) {
                if !pool_bound {
                    pass.set_vertex_buffer(0, self.chunk_vertices.slice(..));
                    pass.set_index_buffer(self.chunk_indices.slice(..), wgpu::IndexFormat::Uint32);
                    pool_bound = true;
                    binds += 1;
                }
                pass.draw_indexed(first..first + count, 0, packed);
                draws += 1;
                triangles += u64::from(count / 3) * copies;
            } else if let Some(mesh) = self.meshes.get(id.0 as usize) {
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pool_bound = false;
                binds += 1;
                pass.draw_indexed(0..mesh.index_count, 0, packed);
                draws += 1;
                triangles += u64::from(mesh.index_count / 3) * copies;
            }
            if counted {
                pass.end_occlusion_query();
            }
            start = end;
        }
        (draws, triangles, binds)
    }

    /// The frame's uniforms with freshly fitted cascades (what `render` uses, without
    /// keeping a far cascade).
    #[cfg(test)]
    pub(crate) fn frame_uniforms(
        &self,
        scene: &SceneView,
    ) -> (FrameUniform, [CascadeUniform; CASCADES]) {
        let view = Mat4::from_cols_array_2d(&scene.camera.view);
        let proj = Mat4::from_cols_array_2d(&scene.camera.proj);
        let sky = SkyModel::new(&scene.sun, &scene.sky);
        let fits = self.fit_shadows(view, proj, sky.sun_dir);
        self.frame_uniforms_with(scene, &sky, &fits)
    }

    pub(crate) fn frame_uniforms_with(
        &self,
        scene: &SceneView,
        sky: &SkyModel,
        fits: &[CascadeFit; CASCADES],
    ) -> (FrameUniform, [CascadeUniform; CASCADES]) {
        let view = Mat4::from_cols_array_2d(&scene.camera.view);
        let proj = Mat4::from_cols_array_2d(&scene.camera.proj);
        let view_proj = proj * view;
        let inv_view = view.inverse();
        let eye = inv_view.w_axis.xyz();
        let forward = -inv_view.z_axis.xyz().normalize_or(Vec3::Z);
        let rs = self.targets.size;
        // The shadow pass draws the same camera-relative instances.
        let to_world = Mat4::from_translation(eye);
        let cascades = fits.map(|f| CascadeUniform {
            view_proj: (f.view_proj * to_world).to_cols_array_2d(),
            _pad: [[0.0; 4]; 12],
        });
        let v4 = |v: Vec3, w: f32| v.extend(w).to_array();
        let moon_dir = Vec3::from(scene.sky.moon_direction).normalize_or(Vec3::NEG_Y);
        let uniform = FrameUniform {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            rel_view_proj: (proj * view_rotation(view)).to_cols_array_2d(),
            cascade_view_proj: fits.map(|f| f.view_proj.to_cols_array_2d()),
            camera_pos: v4(eye, scene.sky.fog_density.max(0.0)),
            camera_fwd: v4(forward, 0.0),
            cascade_far: [
                fits[0].depth_range[1],
                fits[1].depth_range[1],
                fits[2].depth_range[1],
                CASCADE_BLEND,
            ],
            cascade_texel: [
                fits[0].texel,
                fits[1].texel,
                fits[2].texel,
                self.cascade_size as f32,
            ],
            sun_dir: v4(sky.sun_dir, 0.0),
            sun_light: v4(sky.sun_light, 0.0),
            sun_disc: v4(sky.sun_disc, SUN_ANGULAR_RADIUS.cos()),
            moon_dir: v4(moon_dir, MOON_ANGULAR_RADIUS.cos()),
            moon_light: [
                scene.sky.moon_luminance.max(0.0),
                0.01,
                scene.sky.star_brightness.clamp(0.0, 1.0),
                0.0,
            ],
            perez: sky.perez.map(|p| v4(p, 0.0)),
            zenith: v4(sky.zenith, 0.0),
            sun_sky: v4(sky.sun_sky, 0.0),
            sky_params: [sky.day, sky.blue, sky.haze, 0.0],
            night_sky: v4(sky.night, 0.0),
            sky_up: v4(sky.sky_up, 0.0),
            sky_ground: v4(sky.ground, 0.0),
            sky_horizon: v4(sky.horizon, 0.0),
            params: [
                scene.sky.exposure,
                if self.surface_format.is_srgb() {
                    0.0
                } else {
                    1.0
                },
                SHADOW_SUN_OFFSET,
                SHADOW_NORMAL_OFFSET,
            ],
            emissive: [
                scene.sky.emissive_boost.max(0.0),
                scene.sky.window_emissive_scale.clamp(0.0, 1.0),
                scene.lights.len().min(MAX_LIGHTS) as f32,
                scene.sky.emissive_cap.max(1e-3),
            ],
            post: [
                if self.hdr {
                    scene.sky.bloom_strength.max(0.0)
                } else {
                    0.0
                },
                scene.sky.bloom_threshold.max(0.0),
                VIGNETTE,
                BLOOM_CLAMP,
            ],
            screen: [
                self.size.0 as f32,
                self.size.1 as f32,
                1.0 / self.size.0 as f32,
                1.0 / self.size.1 as f32,
            ],
            cabin_light: v4(Vec3::from(scene.sky.cabin_light).max(Vec3::ZERO), 0.0),
            corona: [
                // Pixels (of the scene's target) per unit of view-space slope, vertically.
                0.5 * rs.1 as f32 * proj.y_axis.y.abs(),
                CORONA_MIN_PX * rs.1 as f32 / 720.0,
                CORONA_MAX_PX * rs.1 as f32 / 720.0,
                0.0,
            ],
            render_size: [
                rs.0 as f32,
                rs.1 as f32,
                1.0 / rs.0 as f32,
                1.0 / rs.1 as f32,
            ],
            shadow_params: [
                self.quality.cascades as f32,
                f32::from(u8::from(sky.sun_lit())),
                0.0,
                0.0,
            ],
            lights: std::array::from_fn(|i| {
                scene
                    .lights
                    .get(i)
                    .map_or(PackedLight::zeroed(), PackedLight::new)
            }),
        };
        (uniform, cascades)
    }
}

/// One fullscreen-triangle pass into `target` (no depth).
pub(crate) fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    pipeline: &wgpu::RenderPipeline,
    groups: [&wgpu::BindGroup; 2],
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("fullscreen"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        timestamp_writes,
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, groups[0], &[]);
    pass.set_bind_group(1, groups[1], &[]);
    pass.draw(0..3, 0..1);
}

/// The HUD's `quads` (already in `buffer`) drawn over `target`.
pub(crate) fn hud_pass(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    groups: [&wgpu::BindGroup; 2],
    buffer: &wgpu::Buffer,
    quads: u32,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("hud"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        timestamp_writes,
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, groups[0], &[]);
    pass.set_bind_group(1, groups[1], &[]);
    pass.set_vertex_buffer(0, buffer.slice(..));
    pass.draw(0..4, 0..quads);
}

/// Hermite step from 0 at `e0` to 1 at `e1`, as in WGSL.
pub(crate) fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
