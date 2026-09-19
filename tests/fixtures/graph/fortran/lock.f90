module lock
  use iso_fortran_env
  integer, parameter :: max_holders = 4
contains
  integer function helper()
    helper = 1
  end function helper

  integer function acquire()
    acquire = helper()
  end function acquire
end module lock
