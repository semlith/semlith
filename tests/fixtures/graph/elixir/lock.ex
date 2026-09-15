defmodule Lock do
  import String

  def helper, do: 1

  def acquire, do: helper()
end
