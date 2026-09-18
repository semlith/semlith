variable "region" {
  default = "eu-west-1"
}

module "base" {
  source = "./base"
}

resource "aws_s3_bucket" "lock" {
  bucket = "lock"
}
