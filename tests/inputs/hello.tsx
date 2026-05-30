// Sample TSX file to exercise syntax highlighting, indents,
// and folds in vorto. Open with `vorto assets/samples/hello.tsx`.

import { useState } from "react";

interface GreetingProps {
  name: string;
  initialCount?: number;
}

type Status = "idle" | "active";

function classify(count: number): Status {
  return count > 0 ? "active" : "idle";
}

export function Greeting({ name, initialCount = 0 }: GreetingProps) {
  const [count, setCount] = useState(initialCount);
  const status = classify(count);

  return (
    <section className="greeting" data-status={status}>
      <h1>Hello, {name}!</h1>
      <p>
        You clicked <strong>{count}</strong> times ({status}).
      </p>
      <button type="button" onClick={() => setCount((c) => c + 1)}>
        Increment
      </button>
    </section>
  );
}

export default function App() {
  const people = ["Alice", "Bob", "Carol"];
  return (
    <main>
      {people.map((name) => (
        <Greeting key={name} name={name} initialCount={1} />
      ))}
    </main>
  );
}
