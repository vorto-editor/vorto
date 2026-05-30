<!-- Sample Vue SFC to exercise syntax highlighting, indents,
     and folds in vorto. Open with `vorto assets/samples/hello.vue`. -->
<script setup lang="ts">
import { computed, ref } from "vue";

interface Person {
  name: string;
  age: number;
}

const people = ref<Person[]>([
  { name: "Alice", age: 30 },
  { name: "Bob", age: 17 },
]);

const count = ref(0);
const adults = computed(() => people.value.filter((p) => p.age >= 18));

function increment(): void {
  count.value += 1;
}
</script>

<template>
  <section class="greeting">
    <h1>Hello, Vue!</h1>
    <ul>
      <li v-for="person in adults" :key="person.name">
        {{ person.name }} ({{ person.age }})
      </li>
    </ul>
    <button type="button" @click="increment">Clicked {{ count }} times</button>
  </section>
</template>

<style scoped>
.greeting {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.greeting h1 {
  font-weight: 600;
}
</style>
