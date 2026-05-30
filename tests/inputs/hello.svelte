<!-- Sample Svelte component to exercise syntax highlighting, indents,
     and folds in vorto. Open with `vorto assets/samples/hello.svelte`. -->
<script lang="ts">
  interface Person {
    name: string;
    age: number;
  }

  let people: Person[] = [
    { name: "Alice", age: 30 },
    { name: "Bob", age: 17 },
  ];

  let count = 0;

  $: adults = people.filter((p) => p.age >= 18);

  function increment(): void {
    count += 1;
  }
</script>

<section class="greeting">
  <h1>Hello, Svelte!</h1>

  {#if adults.length > 0}
    <ul>
      {#each adults as person (person.name)}
        <li>{person.name} ({person.age})</li>
      {/each}
    </ul>
  {:else}
    <p>No adults yet.</p>
  {/if}

  <button type="button" on:click={increment}>
    Clicked {count} times
  </button>
</section>

<style>
  .greeting {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
</style>
