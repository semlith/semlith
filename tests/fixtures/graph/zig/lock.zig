const std = @import("std");

fn helper() i32 {
    return 1;
}

fn acquire() i32 {
    return helper();
}
