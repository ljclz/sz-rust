<script setup lang="ts">
import { ref } from "vue";
import Canvas from "@/components/Canvas.vue";

const activePanel = ref("requirement");
const panels = [
  { key: "requirement", label: "panel.requirement" },
  { key: "spec", label: "panel.spec" },
  { key: "design", label: "panel.design" },
  { key: "tasks", label: "panel.tasks" },
  { key: "log", label: "panel.log" },
  { key: "preview", label: "panel.preview" },
  { key: "plugin", label: "panel.plugin" },
];
</script>

<template>
  <div class="workbench">
    <header class="menu-bar">
      <span class="logo">SZ-Rust 画布</span>
      <nav class="menu-nav">
        <button
          v-for="p in panels"
          :key="p.key"
          :class="['menu-item', { active: activePanel === p.key }]"
          @click="activePanel = p.key"
        >
          {{ $t(p.label) }}
        </button>
      </nav>
    </header>
    <main class="content">
      <Canvas :active-panel="activePanel" />
    </main>
    <footer class="status-bar">
      <span>{{ $t("app.title") }}</span>
    </footer>
  </div>
</template>

<style scoped>
.workbench {
  display: flex;
  flex-direction: column;
  height: 100vh;
}
.menu-bar {
  display: flex;
  align-items: center;
  padding: 0 16px;
  height: 48px;
  background: #1e1e1e;
  color: #ccc;
  gap: 16px;
}
.logo {
  font-weight: bold;
  font-size: 14px;
}
.menu-nav {
  display: flex;
  gap: 4px;
}
.menu-item {
  padding: 6px 12px;
  border: none;
  background: transparent;
  color: #aaa;
  cursor: pointer;
  border-radius: 4px;
  font-size: 13px;
}
.menu-item:hover {
  background: #2a2a2a;
  color: #fff;
}
.menu-item.active {
  background: #09977e;
  color: #fff;
}
.content {
  flex: 1;
  overflow: hidden;
}
.status-bar {
  height: 24px;
  background: #1e1e1e;
  color: #888;
  font-size: 12px;
  display: flex;
  align-items: center;
  padding: 0 16px;
}
</style>