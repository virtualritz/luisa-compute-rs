use std::env::current_exe;
use std::f32::consts::PI;

use luisa::prelude::*;
use luisa::Value;
use luisa::{impl_polymorphic, Float};
use luisa_compute as luisa;

trait Area {
    fn area(&self) -> Float;
}
#[derive(Value, Clone, Copy)]
#[repr(C)]
pub struct Circle {
    radius: f32,
}
impl Area for CircleExpr {
    fn area(&self) -> Float {
        PI * self.radius() * self.radius()
    }
}
impl_polymorphic!(Area, Circle);
#[derive(Value, Clone, Copy)]
#[repr(C)]
pub struct Square {
    side: f32,
}
impl Area for SquareExpr {
    fn area(&self) -> Float {
        self.side() * self.side()
    }
}
impl_polymorphic!(Area, Square);
fn main() {
    use luisa::*;
    let ctx = Context::new(current_exe().unwrap());
    let device = ctx.create_device("cpu");
    let circles = device.create_buffer::<Circle>(2);
    circles
        .view(..)
        .copy_from(&[Circle { radius: 1.0 }, Circle { radius: 2.0 }]);
    let squares = device.create_buffer::<Square>(2);
    squares
        .view(..)
        .copy_from(&[Square { side: 1.0 }, Square { side: 2.0 }]);
    // Polymorphic<DevirtualizationKey, Trait>
    // Here we only need the type to devirtualize, so we use `()`.
    let mut poly_area: Polymorphic<(), dyn Area> = Polymorphic::new();
    // since we don't need a key, just supply `()`.
    poly_area.register((), &circles);
    poly_area.register((), &squares);
    let areas = device.create_buffer::<f32>(4);
    let shader = device.create_kernel::<fn()>(&|| {
        let tid = dispatch_id().x();
        let tag = tid / 2;
        let index = tid % 2;
        let area = poly_area
            .get(TagIndexExpr::new(tag, index))
            .dispatch(|_tag, _key, obj| obj.area());
        areas.var().write(tid, area);
    });
    shader.dispatch([4, 1, 1]);
    let areas = areas.view(..).copy_to_vec();
    println!("{:?}", areas);
}
