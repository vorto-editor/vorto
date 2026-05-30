# Sample fish script to exercise syntax highlighting, indents,
# and textobjects in vorto. Open with `vorto assets/samples/hello.fish`.

# Tiny script exercising typical fish constructs:
# functions with named args, if/switch/for/while, string ops,
# command substitution, variable scoping, and abbreviations.

set -gx VORTO_SAMPLE_GREETING "Hello"

function greet --description 'Greet a person, optionally with a custom prefix' --argument-names name prefix
    set -q prefix[1]; or set prefix $VORTO_SAMPLE_GREETING
    echo "$prefix, $name!"
end

function classify --argument-names n
    switch $n
        case -\*
            echo negative
        case 0
            echo zero
        case '*'
            if test (math "$n % 2") -eq 0
                echo "positive even"
            else
                echo "positive odd"
            end
    end
end

set -l people alice bob carol
for name in $people
    greet (string upper -- (string sub --start 1 --length 1 $name))(string sub --start 2 -- $name) "Hi"
end

set -l numbers (seq -- -2 5)
for n in $numbers
    printf '%s is %s\n' $n (classify $n)
end

# Pipelines, command substitution, and arithmetic.
set -l squares
for n in (seq 1 5)
    set -a squares (math "$n * $n")
end
echo "squares: $squares"

# Abbreviation (interactive aid; harmless to define in a script).
abbr -a -- vsample 'vorto assets/samples/hello.fish'

# Conditional with `and`/`or` chaining.
test -d assets/samples
    and echo "samples dir present"
    or echo "samples dir missing"

# String operations.
set -l joined (string join , -- $people)
echo "joined: $joined"

# Exit cleanly.
return 0
