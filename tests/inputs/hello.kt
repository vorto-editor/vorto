// Sample Kotlin file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.kt`.

data class Person(
    val name: String,
    val age: Int,
    val tags: List<String> = emptyList(),
) {
    val isAdult: Boolean
        get() = age >= 18
}

enum class Classification(val label: String) {
    NEGATIVE("negative"),
    ZERO("zero"),
    POSITIVE_EVEN("positive even"),
    POSITIVE_ODD("positive odd"),
}

fun greet(person: Person, prefix: String = "Hello"): String =
    "$prefix, ${person.name}!"

fun classify(n: Int): Classification = when {
    n < 0 -> Classification.NEGATIVE
    n == 0 -> Classification.ZERO
    n % 2 == 0 -> Classification.POSITIVE_EVEN
    else -> Classification.POSITIVE_ODD
}

fun squares(numbers: List<Int>): List<Int> =
    numbers.map { it * it }.filter { it > 5 }

fun main() {
    val alice = Person(name = "Alice", age = 30, tags = listOf("admin", "early_bird"))
    println(greet(alice))

    for (n in -2..5) {
        println("$n -> ${classify(n).label}")
    }

    println("squares: ${squares(listOf(1, 2, 3, 4, 5))}")
}
