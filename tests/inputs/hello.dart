// Sample Dart file to exercise syntax highlighting, indents,
// and textobjects in vorto. Open with `vorto assets/samples/hello.dart`.

/// Tiny program exercising typical Dart constructs:
/// classes, mixins, named/optional params, generics, futures,
/// pattern matching, records, and collection-if/for.
library;

mixin Prefixed {
  String prefix = 'Hello';
}

abstract interface class Greeter<T> {
  String greet(T value);
}

class Person {
  final String name;
  int age;
  final List<String> tags;

  Person({required this.name, this.age = 0, this.tags = const []});

  bool get isAdult => age >= 18;

  @override
  String toString() => 'Person($name, $age)';
}

class PersonGreeter with Prefixed implements Greeter<Person> {
  PersonGreeter({String? prefix}) {
    if (prefix != null) this.prefix = prefix;
  }

  @override
  String greet(Person p) => '$prefix, ${p.name}!';
}

String classify(int n) => switch (n) {
      < 0 => 'negative',
      0 => 'zero',
      _ when n.isEven => 'positive even',
      _ => 'positive odd',
    };

Future<void> main() async {
  final people = <Person>[
    Person(name: 'Alice', age: 30, tags: ['admin']),
    Person(name: 'Bob', age: 17),
    Person(name: 'Carol', age: 42, tags: ['vip', 'early-bird']),
  ];

  final greeter = PersonGreeter(prefix: 'Hi');
  for (final p in people.where((p) => p.isAdult)) {
    print(greeter.greet(p));
  }

  final adultNames = [
    for (final p in people)
      if (p.isAdult) p.name,
  ];
  print('Adults: ${adultNames.join(', ')}');

  for (var n = -2; n <= 5; n++) {
    print('$n is ${classify(n)}');
  }

  final (sum, count) = await _sumAndCount([1, 2, 3, 4]);
  print('sum=$sum count=$count');
}

Future<(int sum, int count)> _sumAndCount(Iterable<int> xs) async {
  await Future<void>.delayed(Duration.zero);
  return (xs.fold(0, (a, b) => a + b), xs.length);
}
