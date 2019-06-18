#[repr(C)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[repr(u32)]
pub enum Foo {
    A = 1,
    B,
    C,
}

#[no_mangle]
pub unsafe extern "C" fn get_origin() -> Point {
    Point { x: 0.0, y: 0.0 }
}

#[no_mangle]
pub unsafe extern "C" fn is_in_range(point: Point, range: f32) -> bool {
    (point.x.powi(2) + point.y.powi(2)).sqrt() <= range
}

#[no_mangle]
pub unsafe extern "C" fn print_foo(foo: *const Foo) {
    println!(
        "{}",
        match *foo {
            Foo::A => "a",
            Foo::B => "b",
            Foo::C => "c",
        }
    );
}
