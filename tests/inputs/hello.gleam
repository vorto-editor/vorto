// Sample Gleam file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.gleam`.

import gleam/int
import gleam/io
import gleam/list
import gleam/string

/// A person with a name, age, and free-form tags.
pub type Person {
  Person(name: String, age: Int, tags: List(String))
}

pub type Classification {
  Negative
  Zero
  PositiveEven
  PositiveOdd
}

const default_greeting = "Hello"

pub fn greet(person: Person, prefix: String) -> String {
  prefix <> ", " <> person.name <> "!"
}

pub fn classify(n: Int) -> Classification {
  case n {
    _ if n < 0 -> Negative
    0 -> Zero
    _ if n % 2 == 0 -> PositiveEven
    _ -> PositiveOdd
  }
}

pub fn squares(numbers: List(Int)) -> List(Int) {
  numbers
  |> list.map(fn(x) { x * x })
  |> list.filter(fn(x) { x > 5 })
}

pub fn main() {
  let alice = Person(name: "Alice", age: 30, tags: ["admin", "early_bird"])
  io.println(greet(alice, default_greeting))
  io.println(greet(alice, "Hi"))

  list.range(-2, 5)
  |> list.each(fn(n) {
    io.println(int.to_string(n) <> " -> " <> string.inspect(classify(n)))
  })

  io.println("squares: " <> string.inspect(squares([1, 2, 3, 4, 5])))
}
