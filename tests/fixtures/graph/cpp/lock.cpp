#include <string>

#define MAX_HOLDERS 4

constexpr int MAX_WAITERS = 8;

int helper() { return 1; }

int acquire() { return helper(); }
