// Sample Odin file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.odin`.

package main

import "core:fmt"
import "core:slice"

Classification :: enum {
	Negative,
	Zero,
	Positive_Even,
	Positive_Odd,
}

Person :: struct {
	name: string,
	age:  int,
	tags: []string,
}

DEFAULT_GREETING :: "Hello"

is_adult :: proc(p: Person) -> bool {
	return p.age >= 18
}

greet :: proc(p: Person, prefix := DEFAULT_GREETING) -> string {
	return fmt.tprintf("%s, %s!", prefix, p.name)
}

classify :: proc(n: int) -> Classification {
	switch {
		case n:
	case n < 0:
		return .Negative
	case n == 0:
		return .Zero
	case n % 2 == 0:
		return .Positive_Even
	case:
		return .Positive_Odd
	}
}

main :: proc() {
	alice := Person {
		name = "Alice",
		age  = 30,
		tags = {"admin", "early_bird"},
	}
	fmt.println(greet(alice))
	fmt.println(greet(alice, "Hi"))

	for n in -2 ..= 5 {
		fmt.printf("%d -> %v\n", n, classify(n))
	}

	squares: [dynamic]int
	defer delete(squares)
	for x in 1 ..= 6 {
		if x * x > 4 {
			append(&squares, x * x)
		}
	}
	fmt.println("squares:", slice.clone(squares[:]))
}
