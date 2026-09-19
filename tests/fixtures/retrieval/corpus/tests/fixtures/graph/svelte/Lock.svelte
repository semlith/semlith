<script>
  import { onMount } from 'svelte';

  function helper() {
    return 1;
  }

  function acquire() {
    return helper();
  }
</script>

<div id="lock">{acquire()}</div>
