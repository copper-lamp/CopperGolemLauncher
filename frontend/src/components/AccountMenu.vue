<script setup lang="ts">
// 账户菜单：头像入口，弹出登录 / 账户信息 / 退出。

import { ref, computed } from "vue";
import { UserRound, LogIn, LogOut, LoaderCircle } from "@lucide/vue";

import { useAccount } from "../composables/useAccount";
import { useI18n } from "../i18n";

const { t } = useI18n();
const { account, loginWaiting, beginLogin, logout } = useAccount();

const open = ref(false);

const avatarText = computed(() => {
  const name = account.value?.gamertag;
  if (!name) return null;
  return name.trim().charAt(0).toUpperCase();
});

function toggle() {
  if (loginWaiting.value) return;
  open.value = !open.value;
}

async function handleLogin() {
  open.value = false;
  await beginLogin();
}

async function handleLogout() {
  open.value = false;
  await logout();
}
</script>

<template>
  <div class="account-menu">
    <button
      class="account-menu__avatar"
      :title="account ? t('account.logged_in_as', { name: account.gamertag }) : t('account.not_logged_in')"
      @click="toggle"
    >
      <LoaderCircle v-if="loginWaiting" :size="16" class="spin" />
      <template v-else-if="avatarText">{{ avatarText }}</template>
      <UserRound v-else :size="16" />
    </button>

    <Transition name="pop">
      <div v-if="open" class="account-menu__panel">
        <template v-if="account">
          <div class="account-menu__info">
            <div class="account-menu__name">{{ account.gamertag }}</div>
            <div v-if="account.xuid" class="account-menu__sub">
              {{ account.xuid }}
            </div>
          </div>
          <button class="account-menu__action" @click="handleLogout">
            <LogOut :size="15" />
            <span>{{ t("account.logout") }}</span>
          </button>
        </template>
        <template v-else>
          <div class="account-menu__info">
            <div class="account-menu__name">{{ t("account.not_logged_in") }}</div>
          </div>
          <button class="account-menu__action" @click="handleLogin">
            <LogIn :size="15" />
            <span>{{ t("account.login_microsoft") }}</span>
          </button>
        </template>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.account-menu {
  position: relative;
  display: flex;
  align-items: center;
}

.account-menu__avatar {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: none;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

.account-menu__avatar:hover {
  background: var(--copper-surface-3);
  color: var(--copper-text);
}

.account-menu__avatar:has(+ .account-menu__panel) {
  box-shadow: 0 0 0 2px var(--copper-accent);
}

.spin {
  animation: spin 1.2s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

.account-menu__panel {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  width: 220px;
  padding: var(--copper-space-2);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  box-shadow: var(--copper-shadow);
  z-index: 100;
}

.account-menu__info {
  padding: var(--copper-space-2) var(--copper-space-3) var(--copper-space-3);
  border-bottom: 1px solid var(--copper-border);
  margin-bottom: var(--copper-space-2);
}

.account-menu__name {
  font-weight: 600;
  font-size: var(--copper-font-size-md);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.account-menu__sub {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.account-menu__action {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  width: 100%;
  height: var(--copper-control-h);
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.account-menu__action:hover {
  background: var(--copper-hover);
}
</style>
