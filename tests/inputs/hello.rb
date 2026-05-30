# Sample Ruby file to exercise syntax highlighting, indents,
# and folds in vorto. Open with `vorto assets/samples/hello.rb`.

# frozen_string_literal: true

module Demo
  CLASSIFICATIONS = {
    negative: "negative",
    zero: "zero",
    positive_even: "positive even",
    positive_odd: "positive odd"
  }.freeze

  class Person
    attr_reader :name, :age, :tags

    def initialize(name, age, tags = [])
      @name = name
      @age = age
      @tags = tags
    end

    def adult?
      age >= 18
    end
  end

  module_function

  def greet(person, prefix: "Hello")
    "#{prefix}, #{person.name}!"
  end

  def classify(n)
    if n.negative?
      CLASSIFICATIONS[:negative]
    elsif n.zero?
      CLASSIFICATIONS[:zero]
    elsif n.even?
      CLASSIFICATIONS[:positive_even]
    else
      CLASSIFICATIONS[:positive_odd]
    end
  end

  def squares(numbers)
    numbers.map { |x| x * x }.select { |x| x > 5 }
  end
end

alice = Demo::Person.new("Alice", 30, %w[admin early_bird])
puts Demo.greet(alice)

(-2..5).each do |n|
  puts "#{n} -> #{Demo.classify(n)}"
end

puts "squares: #{Demo.squares([1, 2, 3, 4, 5]).join(', ')}"
