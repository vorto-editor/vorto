# Sample Julia file to exercise syntax highlighting, indents,
# and folds in vorto. Open with `vorto assets/samples/hello.jl`.

#=
Tiny program exercising typical Julia constructs:
modules, parametric structs, multiple dispatch, abstract types,
comprehensions, broadcasting, string interpolation, macros,
ternaries, ranges, and block comments like this one.
=#

module Greetings

export Person, greet, classify

const DEFAULT_GREETING = "Hello"

abstract type Animal end

struct Person{T<:Integer}
    name::String
    age::T
    tags::Vector{Symbol}
end

Person(name::String, age::Integer) = Person(name, age, Symbol[])

isadult(p::Person) = p.age >= 18

"Greet a person, optionally with a custom prefix."
function greet(p::Person, prefix::AbstractString = DEFAULT_GREETING)
    return "$prefix, $(p.name)!"
end

# Multiple dispatch: a different method for plain strings.
greet(name::AbstractString) = "$DEFAULT_GREETING, $name!"

function classify(n::Integer)
    if n < 0
        :negative
    elseif n == 0
        :zero
    else
        iseven(n) ? :positive_even : :positive_odd
    end
end

squares(range = 1:10) = filter(x -> x > 5, [x^2 for x in range])

function tag_lengths(words)
    return Dict(w => length(w) for w in words)
end

end # module Greetings

using .Greetings

# Script entry — runs with `julia hello.jl`.
alice = Person("Alice", 30, [:admin, :early_bird])
println(greet(alice))
println(greet(alice, "Hi"))
println(greet("Bob"))

for n in -2:5
    println("$n is $(classify(n))")
end

# Broadcasting and a comprehension.
doubled = (1:5) .^ 2
println("squares: ", squares())
println("doubled: ", doubled)
println("lengths: ", Greetings.tag_lengths(["alpha", "beta", "gamma"]))
