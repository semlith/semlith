defmodule Lock do
  import String

  @max_holders 4

  def helper, do: 1

  def acquire, do: helper()
end
