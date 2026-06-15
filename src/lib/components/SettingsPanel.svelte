<script lang="ts">
  import { onMount } from "svelte";
  import { getConfigStatus } from "../api";
  import type { ConfigStatus } from "../types";

  export let onClose: () => void = () => {};

  let status: ConfigStatus | null = null;

  onMount(async () => {
    try {
      status = await getConfigStatus();
    } catch {
      status = { configured: false, provider: "(查询失败)", model: "(查询失败)" };
    }
  });
</script>

<section class="settings" aria-label="设置">
  <h2>设置</h2>
  {#if status}
    <p>Provider: {status.provider}</p>
    <p>状态: {status.configured ? `已配置 (model: ${status.model})` : "未配置"}</p>
    {#if !status.configured}
      <p class="muted">请在 <code>~/.config/sophoni/config.toml</code> 配置 Provider，参考 README。</p>
    {/if}
  {/if}
  <label>当前模型 <input value={status?.model ?? "(未配置)"} readonly /></label>
  <button type="button" on:click={onClose}>关闭</button>
</section>
