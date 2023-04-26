use std::{env::current_exe, time::Instant};

use luisa::init_logger;
#[allow(unused_imports)]
use luisa::prelude::*;
use luisa::*;
use luisa_compute as luisa;
use rand::Rng;
use winit::{
    event::{Event, WindowEvent},
    event_loop::{EventLoop, ControlFlow},
};

const N_GRID: usize = 128;
const N_STEPS: usize = 50;
const N_PARTICLES: usize = N_GRID * N_GRID / 2;
const DX: f32 = 1.0f32 / N_GRID as f32;
const DT: f32 = 1e-4f32;
const P_RHO: f32 = 1.0f32;
const P_VOL: f32 = (DX * 0.5f32) * (DX * 0.5f32);
const P_MASS: f32 = P_RHO * P_VOL;
const GRAVITY: f32 = 9.81f32;
const BOUND: u32 = 3;
const E: f32 = 400.0f32;
const RESOLUTION: u32 = 512;

fn main() {
    init_logger();
    std::env::set_var("WINIT_UNIX_BACKEND", "x11");
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 2 {
        log::info!(
            "Usage: {} <backend>. <backend>: cpu, cuda, dx, metal, remote",
            args[0]
        );
        return;
    }
    let ctx = luisa::Context::new(current_exe().unwrap());
    let device = ctx
        .create_device(if args.len() == 2 {
            args[1].as_str()
        } else {
            "cpu"
        })
        .unwrap();

    let mut rng = rand::thread_rng();
    let x = device
        .create_buffer_from_slice(
            (0..N_PARTICLES)
                .map(|_| {
                    let rx: f32 = rng.gen();
                    let ry: f32 = rng.gen();
                    Float2::new(rx * 0.4f32 + 0.2f32, ry * 0.4f32 + 0.2f32)
                })
                .collect::<Vec<_>>()
                .as_slice(),
        )
        .unwrap();
    let v = device
        .create_buffer_from_slice(&[Float2::new(0.0f32, -1.0f32); N_PARTICLES])
        .unwrap();
    let C = device
        .create_buffer_from_slice(
            &[Mat2 {
                cols: [Float2::new(0.0f32, 0.0f32); 2],
            }; N_PARTICLES],
        )
        .unwrap();
    let J = device
        .create_buffer_from_slice(&[1.0f32; N_PARTICLES])
        .unwrap();
    let grid_v = device.create_buffer::<f32>(N_GRID * N_GRID * 2).unwrap();
    let grid_m = device.create_buffer::<f32>(N_GRID * N_GRID).unwrap();

    let event_loop = EventLoop::new();
    let window = winit::window::WindowBuilder::new()
        .with_title("Luisa Compute Rust - MPM")
        .with_inner_size(winit::dpi::LogicalSize::new(RESOLUTION, RESOLUTION))
        .with_resizable(false)
        .build(&event_loop)
        .unwrap();
    let swapchain = device
        .create_swapchain(
            &window,
            &device.default_stream(),
            RESOLUTION,
            RESOLUTION,
            false,
            false,
            3,
        )
        .unwrap();
    let display = device
        .create_tex2d::<Float4>(swapchain.pixel_storage(), RESOLUTION, RESOLUTION, 1)
        .unwrap();

    let trace = |mat: Expr<Mat2>| -> Expr<f32> { mat.col(0).x() + mat.col(1).y() };

    let index = |xy: Expr<Uint2>| -> Expr<u32> {
        let p = xy.clamp(
            make_uint2(0, 0),
            make_uint2(N_GRID as u32 - 1, N_GRID as u32 - 1),
        );
        p.x() + p.y() * N_GRID as u32
    };

    let clear_grid = device
        .create_kernel_async::<()>(&|| {
            let idx = index(dispatch_id().xy());
            grid_v.var().write(idx * 2, 0.0f32);
            grid_v.var().write(idx * 2 + 1, 0.0f32);
            grid_m.var().write(idx, 0.0f32);
        })
        .unwrap();

    let point_to_grid = device
        .create_kernel_async::<()>(&|| {
            let p = dispatch_id().x();
            let xp = x.var().read(p) / DX;
            let base = (xp - 0.5f32).int();
            let fx = xp - base.float();

            let w = [
                0.5f32 * (1.5f32 - fx) * (1.5f32 - fx),
                0.75f32 - (fx - 1.0f32) * (fx - 1.0f32),
                0.5f32 * (fx - 0.5f32) * (fx - 0.5f32),
            ];
            let stress = -4.0f32 * DT * E * P_VOL * (J.var().read(p) - 1.0f32) / (DX * DX);
            let affine =
                Expr::<Mat2>::eye(make_float2(stress, stress)) + P_MASS as f32 * C.var().read(p);
            let vp = v.var().read(p);
            for ii in 0..9 {
                let (i, j) = (ii % 3, ii / 3);
                let offset = make_int2(i as i32, j as i32);
                let dpos = (offset.float() - fx) * DX;
                let weight = w[i].x() * w[j].y();
                let vadd = weight * (P_MASS * vp + affine * dpos);
                let idx = index((base + offset).uint());
                grid_v.var().atomic_fetch_add(idx * 2, vadd.x());
                grid_v.var().atomic_fetch_add(idx * 2 + 1, vadd.y());
                grid_m.var().atomic_fetch_add(idx, weight * P_MASS);
            }
        })
        .unwrap();

    let simulate_grid = device
        .create_kernel_async::<()>(&|| {
            let coord = dispatch_id().xy();
            let i = index(coord);
            let v = var!(Float2);
            v.store(make_float2(
                grid_v.var().read(i * 2u32),
                grid_v.var().read(i * 2u32 + 1u32),
            ));
            let m = grid_m.var().read(i);

            v.store(select(m.cmpgt(0.0f32), v.load() / m, v.load()));
            let vx = v.load().x();
            let vy = v.load().y() - DT * GRAVITY;
            let vx = select(
                (coord.x().cmplt(BOUND) & vx.cmplt(0.0f32))
                    | (coord.x() + BOUND).cmpgt(N_GRID as u32) & vx.cmpgt(0.0f32),
                0.0f32.into(),
                vx,
            );
            let vy = select(
                (coord.y().cmplt(BOUND) & vy.cmplt(0.0f32))
                    | (coord.y() + BOUND).cmpgt(N_GRID as u32) & vy.cmpgt(0.0f32),
                0.0f32.into(),
                vy,
            );
            grid_v.var().write(i * 2, vx);
            grid_v.var().write(i * 2 + 1, vy);
        })
        .unwrap();

    let grid_to_point = device
        .create_kernel_async::<()>(&|| {
            let p = dispatch_id().x();
            let xp = x.var().read(p) / DX;
            let base = (xp - 0.5f32).int();
            let fx = xp - base.float();

            let w = [
                0.5f32 * (1.5f32 - fx) * (1.5f32 - fx),
                0.75f32 - (fx - 1.0f32) * (fx - 1.0f32),
                0.5f32 * (fx - 0.5f32) * (fx - 0.5f32),
            ];
            let new_v = var!(Float2);
            let new_C = var!(Mat2);
            new_v.store(make_float2(0.0f32, 0.0f32));
            new_C.store(make_float2x2(make_float2(0., 0.), make_float2(0., 0.)));
            for ii in 0..9 {
                let (i, j) = (ii % 3, ii / 3);
                let offset = make_int2(i as i32, j as i32);
                let dpos = (offset.float() - fx) * DX;
                let weight = w[i].x() * w[j].y();
                let idx = index((base + offset).uint());
                let g_v = make_float2(
                    grid_v.var().read(idx * 2u32),
                    grid_v.var().read(idx * 2u32 + 1u32),
                );
                new_v.store(new_v.load() + weight * g_v);
                new_C.store(new_C.load() + 4.0f32 * weight * g_v.outer_product(dpos) / (DX * DX));
            }
            v.var().write(p, new_v);
            x.var().write(p, x.var().read(p) + new_v.load() * DT);
            J.var()
                .write(p, J.var().read(p) * (1.0f32 + DT * trace(new_C.load())));
            C.var().write(p, new_C);
        })
        .unwrap();

    let clear_display = device
        .create_kernel_async::<()>(&|| {
            display.var().write(
                dispatch_id().xy(),
                make_float4(0.1f32, 0.2f32, 0.3f32, 1.0f32),
            );
        })
        .unwrap();
    let draw_particles = device
        .create_kernel_async::<()>(&|| {
            let p = dispatch_id().x();
            for i in -1..=1 {
                for j in -1..=1 {
                    let pos = (x.var().read(p) * RESOLUTION as f32).int() + make_int2(i, j);
                    if_!(
                        pos.x().cmpge(0i32)
                            & pos.x().cmplt(RESOLUTION as i32)
                            & pos.y().cmpge(0i32)
                            & pos.y().cmplt(RESOLUTION as i32),
                        {
                            display.var().write(
                                make_uint2(pos.x().uint(), RESOLUTION - 1u32 - pos.y().uint()),
                                make_float4(0.4f32, 0.6f32, 0.6f32, 1.0f32),
                            );
                        }
                    );
                }
            }
        })
        .unwrap();
    event_loop.run(move |event, _, control_flow| {
        control_flow.set_poll();
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                window_id,
            } if window_id == window.id() => {
                *control_flow = ControlFlow::Exit;
            }
            Event::MainEventsCleared => {
                window.request_redraw();
            }
            Event::RedrawRequested(_) => {
                let tic = Instant::now();
                {
                    let scope = device.default_stream().scope();
                    scope.present(&swapchain, &display).unwrap();
                    let mut commands = Vec::new();
                    for _ in 0 .. N_STEPS {
                        commands.push(clear_grid.dispatch_async([N_GRID as u32, N_GRID as u32, 1]));
                        commands.push(point_to_grid.dispatch_async([N_PARTICLES as u32, 1, 1]));
                        commands.push(simulate_grid.dispatch_async([N_GRID as u32, N_GRID as u32, 1]));
                        commands.push(grid_to_point.dispatch_async([N_PARTICLES as u32, 1, 1]));
                    }
                    commands.push(clear_display.dispatch_async([RESOLUTION as u32, RESOLUTION as u32, 1]));
                    commands.push(draw_particles.dispatch_async([N_PARTICLES as u32, 1, 1]));
                    scope.submit(commands).unwrap();
                }
                let toc = Instant::now();
                let elapsed = (toc - tic).as_secs_f32();
                log::info!("time: {}ms", elapsed * 1e3);
                window.request_redraw();
            }
            _ => (),
        }
    });
}
