<script setup lang="ts">
// 账户菜单：头像入口，弹出登录 / 账户信息 / 退出。

import { ref, computed, watch } from "vue";
import { UserRound, LogOut, LoaderCircle, ShieldCheck } from "@lucide/vue";

import { useAccount } from "../composables/useAccount";
import { useI18n } from "../i18n";

import { useNetworkStatus } from "../composables/useNetworkStatus";
const { connected } = useNetworkStatus();
const { t } = useI18n();
const { account, loginWaiting, beginWamSignIn, logout } = useAccount();

const open = ref(false);

const avatarText = computed(() => {
  const name = account.value?.gamertag;
  if (!name) return null;
  return name.trim().charAt(0).toUpperCase();
});

/** Xbox 档案头像。取不到时回退成首字母，不影响任何登录状态。 */
const avatarUrl = computed(() => account.value?.avatar_url?.trim() || null);
/** 图片加载失败（被墙 / URL 过期）时退回首字母，不留一个破图。 */
const avatarBroken = ref(false);

watch(avatarUrl, () => {
  avatarBroken.value = false;
});

function toggle() {
  if (loginWaiting.value) return;
  open.value = !open.value;
}

async function handleWamSignIn() {
  // 不关闭菜单：系统账户界面就叠加在本窗口上，菜单状态由 finally 复位。
  await beginWamSignIn();
}

async function handleLogout() {
  open.value = false;
  await logout();
}
</script>

<template>
  <div class="account-menu">
    <button
      class="account-menu__trigger"
      :title="account ? t('account.logged_in_as', { name: account.gamertag }) : t('account.login')"
      @click="toggle"
    >
      <span class="account-menu__avatar">
        <LoaderCircle v-if="loginWaiting" :size="16" class="spin" />
        <img
          v-else-if="avatarUrl && !avatarBroken"
          :src="avatarUrl"
          class="account-menu__avatar-img"
          alt=""
          referrerpolicy="no-referrer"
          @error="avatarBroken = true"
        />
        <template v-else-if="avatarText">{{ avatarText }}</template>
        <UserRound v-else :size="16" />
      </span>
      <span class="account-menu__label">
        {{ account ? account.gamertag : t("account.login") }}
      </span>
      <span class="account-menu__network" :class="{ 'account-menu__network--connected': connected === true }" role="img" :title="t(connected === true ? 'account.network_available' : connected === false ? 'account.network_offline' : 'account.network_unknown')" :aria-label="t(connected === true ? 'account.network_available' : connected === false ? 'account.network_offline' : 'account.network_unknown')" />
    </button>

    <Transition name="pop">
      <div v-if="open" class="account-menu__panel">
        <template v-if="account">
          <div class="account-menu__info">
            <span class="account-menu__avatar account-menu__avatar--lg">
              <img
                v-if="avatarUrl && !avatarBroken"
                :src="avatarUrl"
                class="account-menu__avatar-img"
                alt=""
                referrerpolicy="no-referrer"
                @error="avatarBroken = true"
              />
              <template v-else-if="avatarText">{{ avatarText }}</template>
              <UserRound v-else :size="18" />
            </span>
            <span class="account-menu__identity">
              <span class="account-menu__name">{{ account.gamertag }}</span>
              <span v-if="account.xuid" class="account-menu__sub">
                {{ account.xuid }}
              </span>
            </span>
          </div>
          <div class="account-menu__hint">{{ t("account.store_authorized") }}</div>
          <button
            class="account-menu__action"
            :disabled="loginWaiting"
            @click="handleWamSignIn"
          >
            <ShieldCheck :size="15" />
            <span>{{ t("account.store_reauthorize") }}</span>
          </button>
          <button class="account-menu__action" @click="handleLogout">
            <LogOut :size="15" />
            <span>{{ t("account.logout") }}</span>
          </button>
        </template>
        <template v-else>
          <div class="account-menu__info account-menu__info--out">
            <span class="account-menu__identity">
              <span class="account-menu__name">{{ t("account.not_logged_in") }}</span>
              <span class="account-menu__sub">{{ t("account.store_sign_in") }}</span>
            </span>
          </div>
          <button
            class="account-menu__action account-menu__action--primary"
            :disabled="loginWaiting"
            @click="handleWamSignIn"
          >
            <ShieldCheck :size="15" />
            <span>{{ t("account.login") }}</span>
          </button>
        </template>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.account-menu__network { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; background: var(--copper-text-disabled); }
.account-menu__network--connected { background: var(--copper-success); }
.account-menu__label { max-width: 150px; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.account-menu {
  position: relative;
  display: flex;
  align-items: center;
}

.account-menu__trigger {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 10px 0 3px;
  border: none;
  border-radius: var(--copper-radius-full);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.account-menu__trigger:hover {
  background: var(--copper-surface-2);
  color: var(--copper-text);
}

.account-menu__avatar {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  overflow: hidden;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
}

/* 头像铺满圆形：档案图是正方形，裁切比留白更像「头像」。 */
.account-menu__avatar-img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.account-menu__avatar--lg {
  width: 34px;
  height: 34px;
  font-size: var(--copper-font-size-md);
}

.account-menu__label {
  max-width: 120px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--copper-font-size-sm);
}

.account-menu__trigger:has(+ .account-menu__panel) {
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
  /* 240px：唯一的登录按钮文案是「登录 Microsoft 账户（Xbox）」，220px 会折行。 */
  width: 240px;
  padding: var(--copper-space-2);
  background: var(--copper-surface);
  max-width: calc(100vw - 16px);
  border-radius: var(--copper-radius-lg);
  box-shadow: var(--copper-shadow);
  z-index: 100;
}

.account-menu__info {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  min-width: 0;
  padding: var(--copper-space-2) var(--copper-space-3) var(--copper-space-3);
  border-bottom: 1px solid var(--copper-border);
  margin-bottom: var(--copper-space-2);
}

.account-menu__info--out {
  padding: var(--copper-space-2) var(--copper-space-3) var(--copper-space-2);
}

.account-menu__identity {
  display: flex;
  flex-direction: column;
  min-width: 0;
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

.account-menu__hint {
  padding: 0 var(--copper-space-3) var(--copper-space-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  line-height: 1.5;
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

/* 商店授权是下载正版整包的前置条件，视觉上高于设备码登录。
   只用既有主题令牌，避免与主题系统割裂。 */
.account-menu__action--primary {
  background: var(--copper-surface-3);
  color: var(--copper-text);
  font-weight: 500;
}

.account-menu__action--primary:hover {
  background: var(--copper-hover);
}

.account-menu__action:disabled {
  opacity: 0.5;
  cursor: default;
}

.account-menu__action:disabled:hover {
  background: transparent;
}
@media (max-width: 760px) { .account-menu__label { max-width: 75px; } }
@media (max-width: 560px) { .account-menu__label { display: none; } .account-menu__trigger { padding-right: 3px; } }
</style>
