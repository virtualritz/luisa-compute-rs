use luisa_compute as luisa;
use std::env::current_exe;

fn main() {
    use luisa::*;
    init_logger();
    let ctx = Context::new(current_exe().unwrap());
    let device = ctx.create_device("cpu");
    let add = device.create_dyn_callable::<fn(DynExpr, DynExpr) -> DynExpr>(Box::new(
        |a: DynExpr, b: DynExpr| -> DynExpr {
            if let Some(a) = a.downcast::<f32>() {
                let b = b.downcast::<f32>().unwrap();
                return DynExpr::new(a + b);
            } else if let Some(a) = a.downcast::<i32>() {
                let b = b.downcast::<i32>().unwrap();
                return DynExpr::new(a + b);
            } else {
                unreachable!()
            }
        },
    ));
    let x = device.create_buffer::<f32>(1024);
    let y = device.create_buffer::<f32>(1024);
    let z = device.create_buffer::<f32>(1024);
    let w = device.create_buffer::<i32>(1024);
    x.view(..).fill_fn(|i| i as f32);
    y.view(..).fill_fn(|i| 1000.0 * i as f32);
    let kernel = device.create_kernel::<fn(Buffer<f32>)>(&|buf_z| {
        let buf_x = x.var();
        let buf_y = y.var();
        let tid = dispatch_id().x();
        let x = buf_x.read(tid);
        let y = buf_y.read(tid);

        buf_z.write(tid, add.call(x.into(), y.into()).get::<f32>());
        w.var()
            .write(tid, add.call(x.int().into(), y.int().into()).get::<i32>());
    });
    kernel.dispatch([1024, 1, 1], &z);
    let z_data = z.view(..).copy_to_vec();
    println!("{:?}", &z_data[0..16]);
    let w_data = w.view(..).copy_to_vec();
    println!("{:?}", &w_data[0..16]);
}
