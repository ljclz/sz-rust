<script setup lang="ts">
import { useSddStore } from "@/stores/sdd";

const sdd = useSddStore();
</script>

<template>
  <div class="log-panel">
    <h2>{{ $t("panel.log") }}</h2>
    <div class="log-container">
      <div v-for="(event, i) in sdd.events" :key="i" class="log-entry">
        <span class="timestamp">{{ event.timestamp }}</span>
        <span :class="['level', event.eventType]">{{ event.eventType }}</span>
        <span class="message">{{ event.message }}</span>
      </div>
      <div v-if="sdd.events.length === 0" class="empty">暂无日志</div>
    </div>
  </div>
</template>

<style scoped>
.log-panel {
  padding: 20px;
  height: 100%;
  display: flex;
  flex-direction: column;
}
.log-container {
  flex: 1;
  overflow-y: auto;
  background: #1e1e1e;
  padding: 12px;
  border-radius: 4px;
  font-family: monospace;
  font-size: 12px;
}
.log-entry {
  margin-bottom: 4px;
  display: flex;
  gap: 8px;
}
.timestamp {
  color: #666;
}
.level {
  font-weight: bold;
}
.level.phase_start {
  color: #569cd6;
}
.level.phase_complete {
  color: #4ec9b0;
}
.level.error {
  color: #f48771;
}
.message {
  color: #d4d4d4;
}
.empty {
  color: #666;
}
</style>