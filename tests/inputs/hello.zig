// Sample Zig file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.zig`.

const std = @import("std");

const Classification = enum {
    negative,
    zero,
    positive_even,
    positive_odd,

    fn label(self: Classification) []const u8 {
        return switch (self) {
            .negative => "negative",
            .zero => "zero",
            .positive_even => "positive even",
            .positive_odd => "positive odd",
        };
    }
};

const Person = struct {
    name: []const u8,
    age: u32,

    fn isAdult(self: Person) bool {
        return self.age >= 18;
    }
};

fn classify(n: i64) Classification {
    if (n < 0) return .negative;
    if (n == 0) return .zero;
    return if (@mod(n, 2) == 0) .positive_even else .positive_odd;
}

pub fn main() !void {
    const stdout = std.io.getStdOut().writer();
    const alice = Person{ .name = "Alice", .age = 30 };

    try stdout.print("Hello, {s}!\n", .{alice.name});

    var n: i64 = -2;
    while (n <= 5) : (n += 1) {
        try stdout.print("{d} -> {s}\n", .{ n, classify(n).label() });
    }
}
