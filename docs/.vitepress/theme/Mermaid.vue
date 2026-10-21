<script setup lang="ts">
// Draws a ```mermaid block (docs/architecture.md and docs/roadmap.md carry
// several) in the site's own colours, light or dark. Until it has drawn, and
// wherever JavaScript does not run, the highlighted source shows instead.
import { useData } from "vitepress";
import { nextTick, onMounted, ref, watch } from "vue";
import { drawMermaid } from "./draw-mermaid";

const props = defineProps<{ code: string }>();
const { isDark } = useData();
const svg = ref("");

async function draw() {
  await nextTick();
  try {
    svg.value = await drawMermaid(decodeURIComponent(props.code), isDark.value);
  } catch (error) {
    console.warn("vsms: could not draw a mermaid diagram", error);
    svg.value = "";
  }
}

onMounted(draw);
watch(isDark, draw);
</script>

<template>
  <div v-if="svg" class="vsms-mermaid" v-html="svg" />
  <slot v-else />
</template>

<style scoped>
.vsms-mermaid {
  margin: 16px 0;
  overflow-x: auto;
  text-align: center;
  /* Mermaid measured each label outside the page, at the body's line
     height; .vp-doc's taller paragraphs would overflow the boxes. */
  line-height: 24px;
}

/* Edge labels otherwise take a colour mermaid derives for dark mode on its
   own, too dim against this theme's dark surface. */
.vsms-mermaid :deep(.edgeLabel),
.vsms-mermaid :deep(.edgeLabel *) {
  color: var(--md-sys-color-on-surface) !important;
}

.vsms-mermaid :deep(p) {
  margin: 0;
  line-height: inherit;
}
</style>
