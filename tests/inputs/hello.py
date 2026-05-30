# Sample Python file to exercise syntax highlighting, indents,
# and folds in vorto. Open with `vorto assets/samples/hello.py`.

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum


class Classification(Enum):
    NEGATIVE = "negative"
    ZERO = "zero"
    POSITIVE_EVEN = "positive even"
    POSITIVE_ODD = "positive odd"


@dataclass
class Person:
    """A person with a name, age, and free-form tags."""

    name: str
    age: int
    tags: list[str] = field(default_factory=list)

    @property
    def is_adult(self) -> bool:
        return self.age >= 18


def greet(person: Person, prefix: str = "Hello") -> str:
    return f"{prefix}, {person.name}!"


def classify(n: int) -> Classification:
    if n < 0:
        return Classification.NEGATIVE
    if n == 0:
        return Classification.ZERO
    return Classification.POSITIVE_EVEN if n % 2 == 0 else Classification.POSITIVE_ODD


def squares(numbers: list[int]) -> list[int]:
    return [x * x for x in numbers if x * x > 5]


def main() -> None:
    alice = Person(name="Alice", age=30, tags=["admin", "early_bird"])
    print(greet(alice))

    for n in range(-2, 6):
        print(f"{n} -> {classify(n).value}")

    print("squares:", squares([1, 2, 3, 4, 5]))


if __name__ == "__main__":
    main()
