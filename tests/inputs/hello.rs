// Sample Rust file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.rs`.

use std::collections::HashMap;
use std::fmt;

/// A person with a name, age, and free-form tags.
#[derive(Debug, Clone)]
struct Person {
    name: String,
    age: u32,
    tags: Vec<String>,
}

enum Classification {
    Negative,
    Zero,
    PositiveEven,
    PositiveOdd,
}

impl fmt::Display for Classification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Classification::Negative => "negative",
            Classification::Zero => "zero",
            Classification::PositiveEven => "positive even",
            Classification::PositiveOdd => "positive odd",
        };
        write!(f, "{s}")
    }
}

fn greet(person: &Person, prefix: &str) -> String {
    format!("{prefix}, {}!", person.name)
}

fn classify(n: i64) -> Classification {
    match n {
        n if n < 0 => Classification::Negative,
        0 => Classification::Zero,
        n if n % 2 == 0 => Classification::PositiveEven,
        _ => Classification::PositiveOdd,
    }
}

fn squares(numbers: &[i64]) -> Vec<i64> {
    numbers.iter().map(|x| x * x).filter(|x| *x > 5).collect()
}

fn main() {
    let alice = Person {
        name: "Alice".to_string(),
        age: 30,
        tags: vec!["admin".into(), "early_bird".into()],
    };

    println!("{}", greet(&alice, "Hello"));

    let mut counts: HashMap<String, usize> = HashMap::new();
    for n in -2..=5 {
        let label = classify(n).to_string();
        *counts.entry(label).or_insert(0) += 1;
        println!("{n} -> {}", classify(n));
    }

    println!("squares: {:?}", squares(&[1, 2, 3, 4, 5]));
    println!("counts: {counts:?}");
}
