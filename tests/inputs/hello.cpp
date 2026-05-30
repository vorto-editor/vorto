// Sample C++ file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.cpp`.

#include <iostream>
#include <string>
#include <vector>

namespace demo {

enum class Classification {
    Negative,
    Zero,
    PositiveEven,
    PositiveOdd,
};

struct Person {
    std::string name;
    int age{};
    std::vector<std::string> tags;

    [[nodiscard]] bool is_adult() const { return age >= 18; }
};

std::string greet(const Person &p, const std::string &prefix = "Hello") {
    return prefix + ", " + p.name + "!";
}

Classification classify(int n) {
    if (n < 0) return Classification::Negative;
    if (n == 0) return Classification::Zero;
    return n % 2 == 0 ? Classification::PositiveEven : Classification::PositiveOdd;
}

template <typename T>
std::vector<T> squares(const std::vector<T> &xs) {
    std::vector<T> out;
    for (const auto &x : xs) {
        if (x * x > 5) out.push_back(x * x);
    }
    return out;
}

}  // namespace demo

int main() {
    demo::Person alice{"Alice", 30, {"admin", "early_bird"}};
    std::cout << demo::greet(alice) << '\n';

    for (int n = -2; n <= 5; ++n) {
        std::cout << n << " is "
                  << (demo::classify(n) == demo::Classification::Zero ? "zero" : "nonzero")
                  << '\n';
    }
    return 0;
}
