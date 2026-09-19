<template>
  <div id="lock">{{ acquire() }}</div>
</template>

<script>
import base from './base.js';

export default {
  methods: {
    helper() {
      return 1;
    },
    acquire() {
      return this.helper();
    },
  },
};
</script>
