#include <stdio.h>

int helper() { return 1; }

int acquire() { return helper(); }
