# Sample Elixir file to exercise syntax highlighting, indents,
# and textobjects in vorto. Open with `vorto assets/samples/hello.ex`.

defmodule Greeter do
  @moduledoc """
  Tiny greeter module to show typical Elixir constructs:
  module attributes, structs, pattern matching, pipes,
  guards, sigils, and comprehensions.
  """

  @default_greeting "Hello"

  defstruct [:name, age: 0, tags: []]

  @typedoc "A person with a name, age, and free-form tags."
  @type t :: %__MODULE__{name: String.t(), age: non_neg_integer(), tags: [atom()]}

  @doc "Greet a person, optionally with a custom prefix."
  @spec greet(t(), String.t()) :: String.t()
  def greet(%__MODULE__{name: name}, prefix \\ @default_greeting)
      when is_binary(prefix) and prefix != "" do
    "#{prefix}, #{name}!"
  end

  def classify(n) when is_integer(n) do
    cond do
      n < 0 -> :negative
      n == 0 -> :zero
      rem(n, 2) == 0 -> :positive_even
      true -> :positive_odd
    end
  end

  def squares(range \\ 1..10) do
    range
    |> Enum.map(&(&1 * &1))
    |> Enum.filter(fn n -> n > 5 end)
    |> Enum.with_index()
  end

  def tag_words do
    for word <- ~w(alpha beta gamma), into: %{} do
      {word, String.length(word)}
    end
  end
end

defprotocol Shoutable do
  @doc "Convert a thing to a SHOUTING string."
  def shout(value)
end

defimpl Shoutable, for: BitString do
  def shout(s), do: String.upcase(s) <> "!!!"
end

# Script entry — runs when executed with `elixir hello.ex`.
alice = %Greeter{name: "Alice", age: 30, tags: [:admin, :early_bird]}
IO.puts(Greeter.greet(alice))
IO.puts(Greeter.greet(alice, "Hi"))
IO.inspect(Greeter.classify(-3))
IO.inspect(Greeter.squares())
IO.inspect(Greeter.tag_words())
IO.puts(Shoutable.shout("hello"))
