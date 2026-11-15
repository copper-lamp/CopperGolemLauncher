<script setup lang="ts">
import { ref, computed, watch } from "vue";
import { UserRound, LogOut, LoaderCircle, ShieldCheck } from "@lucide/vue";
import { useAccount } from "../composables/useAccount";
import { useI18n } from "../i18n";
import { useNetworkStatus } from "../composables/useNetworkStatus";

const { t } = useI18n();
const { connected } = useNetworkStatus();
const { account, loginWaiting, beginLogin, logout } = useAccount();
const open = ref(false);
const avatarBroken = ref(false);
const avatarUrl = computed(() => account.value?.avatar_url?.trim() || null);
const avatarText = computed(() => account.value?.gamertag?.trim().charAt(0).toUpperCase() || null);
watch(avatarUrl, () => { avatarBroken.value = false; });

function toggle() {
  if (!loginWaiting.value) open.value = !open.value;
}
async function signOut() {
  open.value = false;
  await logout();
}
</script>

<template>
  <div class="android-account">
    <button class="android-account__trigger" :aria-expanded="open" :title="t('account.android.title')" @click="toggle">
      <LoaderCircle v-if="loginWaiting" :size="16" class="spin" />
      <img v-else-if="avatarUrl && !avatarBroken" :src="avatarUrl" alt="" referrerpolicy="no-referrer" @error="avatarBroken = true" />
      <span v-else-if="avatarText" class="android-account__initial">{{ avatarText }}</span>
      <UserRound v-else :size="18" />
      <span>{{ t("account.android.title") }}</span>
      <span class="android-account__network" :class="{ 'android-account__network--online': connected === true }" :aria-label="t(connected === true ? 'account.network_available' : connected === false ? 'account.network_offline' : 'account.network_unknown')" />
    </button>
    <Transition name="pop">
      <section v-if="open" class="android-account__panel" :aria-label="t('account.android.title')">
        <header class="android-account__heading">{{ t("account.android.title") }}</header>
        <section class="android-account__provider">
          <div class="android-account__provider-head">
            <div class="android-account__identity">
              <strong>{{ t("account.android.play_title") }}</strong>
              <span class="android-account__muted">{{ t("account.android.play_status") }}</span>
            </div>
            <span class="android-account__status">{{ t("account.android.unavailable") }}</span>
          </div>
          <p class="android-account__hint">{{ t("account.android.play_unavailable") }}</p>
        </section>
        <section class="android-account__provider">
          <div class="android-account__provider-head">
            <span class="android-account__avatar">
              <img v-if="account && avatarUrl && !avatarBroken" :src="avatarUrl" alt="" referrerpolicy="no-referrer" @error="avatarBroken = true" />
              <span v-else-if="account && avatarText">{{ avatarText }}</span>
              <UserRound v-else :size="17" />
            </span>
            <div class="android-account__identity">
              <strong>{{ t("account.android.xbox_title") }}</strong>
              <span class="android-account__muted">{{ account ? account.gamertag : t("account.not_logged_in") }}</span>
            </div>
            <span class="android-account__status" :class="{ 'android-account__status--active': account }">
              {{ t(account ? "account.android.signed_in" : "account.not_logged_in") }}
            </span>
          </div>
          <div v-if="account?.xuid" class="android-account__detail"><span>{{ t("account.android.xuid") }}</span><code>{{ account.xuid }}</code></div>
          <button v-if="!account" class="android-account__action android-account__action--primary" :disabled="loginWaiting" @click="beginLogin">
            <LoaderCircle v-if="loginWaiting" :size="16" class="spin" />
            <ShieldCheck v-else :size="16" />
            <span>{{ t("account.android.xbox_sign_in") }}</span>
          </button>
          <button v-else class="android-account__action" :disabled="loginWaiting" @click="signOut">
            <LogOut :size="16" />
            <span>{{ t("account.logout") }}</span>
          </button>
        </section>
      </section>
    </Transition>
  </div>
</template>

<style scoped>
.android-account { position: relative; display: flex; align-items: center; }
.android-account__trigger { display: flex; align-items: center; gap: 7px; height: 34px; padding: 0 10px; border: 0; border-radius: var(--copper-radius-full); background: transparent; color: var(--copper-text-secondary); font-size: var(--copper-font-size-sm); cursor: pointer; }
.android-account__trigger:hover { background: var(--copper-surface-2); color: var(--copper-text); }
.android-account__trigger img, .android-account__initial { width: 24px; height: 24px; border-radius: 50%; object-fit: cover; background: var(--copper-surface-2); display: grid; place-items: center; }
.android-account__network { width: 7px; height: 7px; flex: 0 0 7px; border-radius: 50%; background: var(--copper-text-disabled); }
.android-account__network--online { background: var(--copper-success); }
.android-account__panel { position: absolute; z-index: 100; top: calc(100% + 6px); right: 0; width: min(340px, calc(100vw - 16px)); padding: var(--copper-space-3); border-radius: var(--copper-radius-card); background: var(--copper-surface); box-shadow: var(--copper-shadow-card); }
.android-account__heading { padding: 2px var(--copper-space-2) var(--copper-space-3); font-size: var(--copper-font-size-md); font-weight: 650; }
.android-account__provider { padding: var(--copper-space-3) var(--copper-space-2); border-radius: var(--copper-radius-md); background: var(--copper-surface-2); }
.android-account__provider + .android-account__provider { margin-top: var(--copper-space-2); }
.android-account__provider-head { display: flex; align-items: center; gap: var(--copper-space-2); min-width: 0; }
.android-account__identity { display: flex; flex: 1; flex-direction: column; min-width: 0; gap: 3px; }
.android-account__identity strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--copper-font-size-sm); }
.android-account__muted, .android-account__detail { color: var(--copper-text-secondary); font-size: var(--copper-font-size-xs); }
.android-account__status { flex-shrink: 0; color: var(--copper-text-secondary); font-size: var(--copper-font-size-xs); }
.android-account__status--active { color: var(--copper-success); }
.android-account__hint { margin: var(--copper-space-2) 0 0; color: var(--copper-text-secondary); font-size: var(--copper-font-size-xs); line-height: 1.5; }
.android-account__avatar { display: grid; flex: 0 0 30px; width: 30px; height: 30px; place-items: center; overflow: hidden; border-radius: 50%; background: var(--copper-surface-3); }
.android-account__avatar img { width: 100%; height: 100%; object-fit: cover; }
.android-account__detail { display: flex; justify-content: space-between; gap: var(--copper-space-2); margin-top: var(--copper-space-2); }
.android-account__detail code { overflow: hidden; text-overflow: ellipsis; }
.android-account__action { display: flex; align-items: center; justify-content: center; gap: var(--copper-space-2); width: 100%; min-height: var(--copper-control-h); margin-top: var(--copper-space-2); padding: 0 var(--copper-space-2); border: 0; border-radius: var(--copper-radius-md); background: transparent; color: var(--copper-text); cursor: pointer; }
.android-account__action:hover { background: var(--copper-hover); }
.android-account__action--primary { background: var(--copper-surface-3); font-weight: 550; }
.android-account__action:disabled { opacity: .55; cursor: default; }
.spin { animation: spin 1.2s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .spin { animation-duration: 2.4s; } }
</style>
