<!--
  Svelte component sample exercising the constructs the svelte
  highlight query targets: tags, attributes, control-flow blocks
  (`{#if}` / `{:else}` / `{/if}`, `{#each}`, `{#await}`, `{#key}`,
  `{@const}`, `{@html}`, `{@debug}`), snippet declarations and
  invocations (`{#snippet}` / `{@render}`), and bound/expression
  attributes. Embedded `<script>` / `<style>` blocks render as
  plain text until injection is wired up.
-->
<script lang="ts">
  let count = $state(0)
  let items = $state(['alpha', 'beta', 'gamma'])
  let promise = fetch('/api/things').then(r => r.json())

  const greeting = 'world'
  function increment() { count++ }
</script>

<div class="wrapper" class:active={count > 0}>
  {@const doubled = count * 2}

  <h1>Hello {greeting}!</h1>
  <button on:click={increment} disabled={count >= 10}>
    Clicks: {count} (×2 = {doubled})
  </button>

  {#if count > 5}
    <p>Big number</p>
  {:else if count > 0}
    <p>Small number</p>
  {:else}
    <p>Click me</p>
  {/if}

  <ul>
    {#each items as item, i (item)}
      <li>{i + 1}. {item}</li>
    {:else}
      <li>(no items)</li>
    {/each}
  </ul>

  {#await promise}
    <p>loading…</p>
  {:then data}
    <pre>{JSON.stringify(data, null, 2)}</pre>
  {:catch err}
    <p style:color="red">error: {err.message}</p>
  {/await}

  {#key count}
    <span>resets when count changes</span>
  {/key}

  {#snippet row(label, idx)}
    <li>row #{idx}: {label}</li>
  {/snippet}

  {@render row('first', 0)}
  {@render row('second', 1)}

  {@html '<em>raw html allowed</em>'}
  {@debug count}
</div>

<style>
  .wrapper { padding: 1rem; }
  .wrapper.active { border: 1px solid dodgerblue; }
</style>
