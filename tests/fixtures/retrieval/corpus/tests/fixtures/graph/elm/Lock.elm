module Lock exposing (acquire)

import String


helper : Int
helper =
    1


acquire : Int
acquire =
    helper
