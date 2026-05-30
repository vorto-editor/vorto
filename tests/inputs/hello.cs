// Sample C# file to exercise syntax highlighting, indents,
// and textobjects in vorto. Open with `vorto assets/samples/hello.cs`.

using System;
using System.Collections.Generic;
using System.Linq;

namespace VortoSample;

/// <summary>
/// A tiny program that exercises typical C# constructs:
/// records, properties, pattern matching, LINQ, generics, and async.
/// </summary>
public record Person(string Name, int Age)
{
    public string Greeting => $"Hello, {Name}!";
}

public interface IGreeter<T>
{
    string Greet(T value);
}

public class PersonGreeter : IGreeter<Person>
{
    private const string DefaultPrefix = "Hello";

    public string Prefix { get; init; } = DefaultPrefix;

    public string Greet(Person p) => $"{Prefix}, {p.Name}!";
}

public static class Classifier
{
    public static string Classify(int n) => n switch
    {
        < 0 => "negative",
        0 => "zero",
        var x when x % 2 == 0 => "positive even",
        _ => "positive odd",
    };
}

public static class Program
{
    public static async Task Main()
    {
        var people = new List<Person>
        {
            new("Alice", 30),
            new("Bob", 25),
            new("Carol", 42),
        };

        var greeter = new PersonGreeter { Prefix = "Hi" };
        foreach (var p in people)
        {
            Console.WriteLine(greeter.Greet(p));
        }

        var adults = people
            .Where(p => p.Age >= 21)
            .OrderByDescending(p => p.Age)
            .Select(p => p.Name)
            .ToList();

        Console.WriteLine($"Adults: {string.Join(", ", adults)}");

        for (var n = -2; n <= 5; n++)
        {
            Console.WriteLine($"{n} is {Classifier.Classify(n)}");
        }

        await Task.Delay(0);
    }
}
