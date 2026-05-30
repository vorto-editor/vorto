<?php
// Sample PHP file to exercise syntax highlighting, indents,
// and textobjects in vorto. Open with `vorto assets/samples/hello.php`.

declare(strict_types=1);

namespace Vorto\Sample;

/**
 * Tiny program exercising typical PHP constructs:
 * namespaces, enums, readonly properties, interfaces,
 * traits, match expressions, arrow functions, and named args.
 */
enum Classification: string
{
    case Negative = 'negative';
    case Zero = 'zero';
    case PositiveEven = 'positive even';
    case PositiveOdd = 'positive odd';

    public static function of(int $n): self
    {
        return match (true) {
            $n < 0 => self::Negative,
            $n === 0 => self::Zero,
            $n % 2 === 0 => self::PositiveEven,
            default => self::PositiveOdd,
        };
    }
}

interface Greeter
{
    public function greet(Person $p): string;
}

trait Prefixed
{
    public string $prefix = 'Hello';
}

final readonly class Person
{
    public function __construct(
        public string $name,
        public int $age,
        public array $tags = [],
    ) {}
}

final class PersonGreeter implements Greeter
{
    use Prefixed;

    public function greet(Person $p): string
    {
        return "{$this->prefix}, {$p->name}!";
    }
}

$people = [
    new Person(name: 'Alice', age: 30, tags: ['admin']),
    new Person(name: 'Bob', age: 25),
    new Person(name: 'Carol', age: 42, tags: ['vip', 'early-bird']),
];

$greeter = new PersonGreeter();
$greeter->prefix = 'Hi';

foreach ($people as $p) {
    echo $greeter->greet($p), PHP_EOL;
}

$adultNames = array_map(
    fn (Person $p) => $p->name,
    array_filter($people, fn (Person $p) => $p->age >= 21),
);
echo 'Adults: ', implode(', ', $adultNames), PHP_EOL;

for ($n = -2; $n <= 5; $n++) {
    printf("%d is %s%s", $n, Classification::of($n)->value, PHP_EOL);
}
