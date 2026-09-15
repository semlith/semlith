using System;

class Lock
{
    void Helper() {}

    void Acquire() { Helper(); }
}
