// Sample Scala file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.scala`.

package demo

/** A tiny program exercising typical Scala constructs:
  *  case classes, enums, traits, pattern matching, given/using,
  *  for-comprehensions, and string interpolation.
  */

enum Classification:
  case Negative, Zero, PositiveEven, PositiveOdd

final case class Person(name: String, age: Int, tags: List[Symbol] = Nil):
  def isAdult: Boolean = age >= 18

trait Greeter[A]:
  def greet(subject: A): String

object Greetings:
  val DefaultGreeting = "Hello"

  given Greeter[Person] with
    def greet(p: Person): String = s"$DefaultGreeting, ${p.name}!"

  def classify(n: Int): Classification =
    n match
      case x if x < 0      => Classification.Negative
      case 0               => Classification.Zero
      case x if x % 2 == 0 => Classification.PositiveEven
      case _               => Classification.PositiveOdd

  def squares(xs: Seq[Int]): Seq[Int] =
    for
      x <- xs
      sq = x * x
      if sq > 4
    yield sq

@main def run(): Unit =
  val people = List(
    Person("Alice", 30, List(Symbol("admin"))),
    Person("Bob", 17),
  )
  val greeter = summon[Greeter[Person]]
  people.filter(_.isAdult).foreach(p => println(greeter.greet(p)))

  (-2 to 5).foreach(n => println(s"$n -> ${Greetings.classify(n)}"))
  println(s"squares: ${Greetings.squares(1 to 6)}")
