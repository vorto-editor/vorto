<!--
  Vue single-file component sample exercising the template-layer
  constructs the vue highlight query targets: tags, attributes,
  quoted strings, mustache interpolation, and the full directive
  family — `v-if`, `:prop` (bind shorthand), `@event` (on shorthand),
  `#slot`, and modifier chains. Embedded `<script>` / `<style>`
  blocks render as plain text until injection is wired up.
-->
<template>
  <div class="card" :class="{ active: isActive }" v-if="visible">
    <header>
      <h1>{{ title }}</h1>
      <button @click.prevent="onClose" :disabled="busy">Close</button>
    </header>

    <ul v-for="(item, idx) in items" :key="item.id">
      <li @click="select(item)">
        <slot name="row" :item="item" :index="idx">
          Default row for {{ item.name }}
        </slot>
      </li>
    </ul>

    <input v-model.trim="query" placeholder="Search…" />

    <template #footer="{ count }">
      <p>{{ count }} item(s)</p>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'

const props = defineProps<{ items: Array<{ id: string; name: string }> }>()
const visible = ref(true)
const busy = ref(false)
const query = ref('')
const isActive = computed(() => query.value.length > 0)

function select(item: { id: string }) {
  console.log('selected', item.id)
}

function onClose() {
  visible.value = false
}
</script>

<style scoped>
.card {
  border: 1px solid #ccc;
  padding: 1rem;
}
.card.active {
  border-color: dodgerblue;
}
</style>
