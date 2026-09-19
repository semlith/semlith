import scala.collection.mutable

object Lock {
  val MaxHolders = 4

  def helper(): Int = 1

  def acquire(): Int = helper()
}
