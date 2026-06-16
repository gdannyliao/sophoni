<script lang="ts">
  export let content: string;
</script>

<div class="message-bubble" data-testid="message-bubble">
  {content}
</div>

<style>
  .message-bubble {
    align-self: flex-end;
    max-width: 70%;
    background: var(--accent-bg);
    color: white;
    padding: var(--space-2) var(--space-4);
    border-radius: var(--radius-lg) var(--radius-lg) var(--radius-sm) var(--radius-lg);
    word-break: break-word;
  }
</style>
