// Sample JavaScript file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.js`.

class Person {
  constructor(name, age, tags = []) {
    this.name = name;
    this.age = age;
    this.tags = tags;
  }

  get isAdult() {
    return this.age >= 18;
  }
}

const CLASSES = Object.freeze({
  NEGATIVE: "negative",
  ZERO: "zero",
  POSITIVE_EVEN: "positive even",
  POSITIVE_ODD: "positive odd",
});

function greet(person, prefix = "Hello") {
  return `${prefix}, ${person.name}!`;
}

function classify(n) {
  if (n < 0) return CLASSES.NEGATIVE;
  if (n === 0) return CLASSES.ZERO;
  return n % 2 === 0 ? CLASSES.POSITIVE_EVEN : CLASSES.POSITIVE_ODD;
}

const squares = (numbers) => numbers.map((x) => x * x).filter((x) => x > 5);

function main() {
  const alice = new Person("Alice", 30, ["admin", "early_bird"]);
  console.log(greet(alice));

  for (let n = -2; n <= 5; n++) {
    console.log(`${n} -> ${classify(n)}`);
  }

  console.log("squares:", squares([1, 2, 3, 4, 5]));
}

main();
