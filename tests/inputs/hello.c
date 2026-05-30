/* Sample C file to exercise syntax highlighting, indents,
 * and folds in vorto. Open with `vorto assets/samples/hello.c`. */

#include <stdio.h>
#include <string.h>

#define MAX_TAGS 8

typedef enum {
    NEGATIVE,
    ZERO,
    POSITIVE_EVEN,
    POSITIVE_ODD,
} Classification;

typedef struct {
    const char *name;
    int age;
    const char *tags[MAX_TAGS];
    int tag_count;
} Person;

static const char *classify_name(Classification c) {
    switch (c) {
        case NEGATIVE:      return "negative";
        case ZERO:          return "zero";
        case POSITIVE_EVEN: return "positive even";
        default:            return "positive odd";
    }
}

static Classification classify(int n) {
    if (n < 0) return NEGATIVE;
    if (n == 0) return ZERO;
    return (n % 2 == 0) ? POSITIVE_EVEN : POSITIVE_ODD;
}

int main(void) {
    Person alice = {
        .name = "Alice",
        .age = 30,
        .tags = {"admin", "early_bird"},
        .tag_count = 2,
    };

    printf("Hello, %s!\n", alice.name);

    for (int n = -2; n <= 5; n++) {
        printf("%d -> %s\n", n, classify_name(classify(n)));
    }

    return 0;
}
