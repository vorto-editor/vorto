// Sample Go file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.go`.

package main

import (
	"fmt"
	"strings"
)

// Person has a name, age, and free-form tags.
type Person struct {
	Name string
	Age  int
	Tags []string
}

type Classification int

const (
	Negative Classification = iota
	Zero
	PositiveEven
	PositiveOdd
)

func (c Classification) String() string {
	switch c {
	case Negative:
		return "negative"
	case Zero:
		return "zero"
	case PositiveEven:
		return "positive even"
	default:
		return "positive odd"
	}
}

func greet(p Person, prefix string) string {
	return fmt.Sprintf("%s, %s!", prefix, p.Name)
}

func classify(n int) Classification {
	switch {
	case n < 0:
		return Negative
	case n == 0:
		return Zero
	case n%2 == 0:
		return PositiveEven
	default:
		return PositiveOdd
	}
}

func main() {
	alice := Person{Name: "Alice", Age: 30, Tags: []string{"admin", "early_bird"}}
	fmt.Println(greet(alice, "Hello"))

	labels := make([]string, 0, 8)
	for n := -2; n <= 5; n++ {
		labels = append(labels, classify(n).String())
	}
	fmt.Println(strings.Join(labels, ", "))
}
