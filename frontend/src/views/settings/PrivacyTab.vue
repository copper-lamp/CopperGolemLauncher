<script setup lang="ts">
import { useSettingsText } from '../../services/settings-categories';
const st=useSettingsText();
</script>
<template><section class="privacy"><h2>{{st('privacy')}}</h2><p>{{st('privacyBody')}}</p></section></template>
<style scoped>.privacy{padding:var(--copper-space-5);background:var(--copper-surface);border-radius:var(--copper-radius-lg);box-shadow:var(--copper-shadow-sm)}p{color:var(--copper-text-secondary);line-height:1.7}</style>
