using System;

class Lock
{
    const int MaxHolders = 4;

    void Helper() {}

    void Acquire() { Helper(); }
}
