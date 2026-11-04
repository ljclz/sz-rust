<script setup lang="ts">
import { ref, watch } from "vue";
import { useSddStore } from "@/stores/sdd";

const sdd = useSddStore();
const tasks = ref<{ id: string; title: string; status: string }[]>([]);

watch(
  () => sdd.session?.artifacts?.task,
  async () => {
    if (sdd.session) {
      try {
        const raw = await sdd.readArtifact(sdd.session.sessionId, "task");
        tasks.value = parseTasks(raw);
      } catch {
        tasks.value = [];
      }
    }
  },
  { immediate: true },
);

function parseTasks(raw: string): { id: string; title: string; status: string }[] {
  const lines = raw.split("\n").filter((l) => l.trim().startsWith("- ["));
  return lines.map((line, i) => ({
    id: String(i + 1),
    title: line.replace(/- \[[ x]\]/, "").trim(),
    status: line.includes("[x]") ? "done" : "todo",
  }));
}
</script>

<template>
  <div class="task-board">
    <h2>{{ $t("panel.tasks") }}</h2>
    <div v-if="tasks.length === 0" class="empty">暂无任务</div>
    <div v-else class="columns">
      <div class="column">
        <h3>待办</h3>
        <div v-for="t in tasks.filter((x) => x.status === 'todo')" :key="t.id" class="card">
          {{ t.title }}
        </div>
      </div>
      <div class="column">
        <h3>已完成</h3>
        <div v-for="t in tasks.filter((x) => x.status === 'done')" :key="t.id" class="card done">
          {{ t.title }}
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.task-board {
  padding: 20px;
}
.empty {
  color: #888;
}
.columns {
  display: flex;
  gap: 16px;
}
.column {
  flex: 1;
}
.card {
  padding: 8px 12px;
  background: #2d2d2d;
  border-radius: 4px;
  margin-bottom: 8px;
  font-size: 13px;
}
.card.done {
  opacity: 0.6;
  text-decoration: line-through;
}
</style>