import scala.collection.mutable

object Lock {
  def helper(): Int = 1

  def acquire(): Int = helper()
}
