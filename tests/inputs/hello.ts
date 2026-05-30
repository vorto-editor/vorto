// Sample TypeScript file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.ts`.

interface Person {
  name: string;
  age: number;
  tags: string[];
}

type Classification = "negative" | "zero" | "positive even" | "positive odd";

enum Level {
  Low,
  Medium,
  High,
}

function greet(person: Person, prefix = "Hello"): string {
  return `${prefix}, ${person.name}!`;
}

function classify(n: number): Classification {
  if (n < 0) return "negative";
  if (n === 0) return "zero";
  return n % 2 === 0 ? "positive even" : "positive odd";
}

const squares = (numbers: number[]): number[] =>
  numbers.map((x) => x * x).filter((x) => x > 5);

function main(): void {
  const alice: Person = {
    name: "Alice",
    age: 30,
    tags: ["admin", "early_bird"],
  };

  console.log(greet(alice));

  for (let n = -2; n <= 5; n++) {
    console.log(`${n} -> ${classify(n)}`);
  }

  console.log("squares:", squares([1, 2, 3, 4, 5]));
  console.log("level:", Level[Level.High]);
}

main();
