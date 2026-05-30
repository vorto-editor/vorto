// Sample Java file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.java`.

import java.util.List;
import java.util.stream.Collectors;

public class hello {

    enum Classification {
        NEGATIVE("negative"),
        ZERO("zero"),
        POSITIVE_EVEN("positive even"),
        POSITIVE_ODD("positive odd");

        final String label;

        Classification(String label) {
            this.label = label;
        }
    }

    record Person(String name, int age, List<String> tags) {
        boolean isAdult() {
            return age >= 18;
        }
    }

    static String greet(Person person, String prefix) {
        return "%s, %s!".formatted(prefix, person.name());
    }

    static Classification classify(int n) {
        if (n < 0) return Classification.NEGATIVE;
        if (n == 0) return Classification.ZERO;
        return n % 2 == 0 ? Classification.POSITIVE_EVEN : Classification.POSITIVE_ODD;
    }

    public static void main(String[] args) {
        var alice = new Person("Alice", 30, List.of("admin", "early_bird"));
        System.out.println(greet(alice, "Hello"));

        var labels = java.util.stream.IntStream.rangeClosed(-2, 5)
                .mapToObj(n -> classify(n).label)
                .collect(Collectors.joining(", "));
        System.out.println(labels);
    }
}
