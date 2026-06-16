<script lang="ts">
  export let title: string;
</script>

<div class="thought-line" data-testid="thought-line">
  💭 {title}
</div>

<style>
  .thought-line {
    color: var(--text-secondary);
    font-size: 13px;
    padding-left: var(--space-2);
  }
</style>
