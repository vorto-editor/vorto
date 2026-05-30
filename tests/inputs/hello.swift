// Sample Swift file to exercise syntax highlighting, indents,
// and textobjects in vorto. Open with `vorto assets/samples/hello.swift`.

import Foundation

/// A tiny program exercising typical Swift constructs:
/// structs, enums with associated values, protocols, generics,
/// guards, optionals, closures, and string interpolation.

protocol Greeter {
    associatedtype Subject
    func greet(_ subject: Subject) -> String
}

struct Person: Equatable {
    let name: String
    var age: Int

    var isAdult: Bool { age >= 18 }
}

enum Classification: CustomStringConvertible {
    case negative
    case zero
    case positive(even: Bool)

    var description: String {
        switch self {
        case .negative: return "negative"
        case .zero: return "zero"
        case .positive(let even): return even ? "positive even" : "positive odd"
        }
    }
}

struct PersonGreeter: Greeter {
    var prefix: String = "Hello"

    func greet(_ subject: Person) -> String {
        "\(prefix), \(subject.name)!"
    }
}

func classify(_ n: Int) -> Classification {
    guard n != 0 else { return .zero }
    if n < 0 { return .negative }
    return .positive(even: n.isMultiple(of: 2))
}

func summarize<T: Sequence>(_ xs: T) -> String where T.Element == Int {
    let mapped = xs.map { $0 * $0 }.filter { $0 > 4 }
    return mapped.map(String.init).joined(separator: ", ")
}

let people = [
    Person(name: "Alice", age: 30),
    Person(name: "Bob", age: 17),
    Person(name: "Carol", age: 42),
]

let greeter = PersonGreeter(prefix: "Hi")
for p in people where p.isAdult {
    print(greeter.greet(p))
}

for n in -2...5 {
    print("\(n) is \(classify(n))")
}

print("squares > 4: \(summarize(1...6))")
