# Sample HCL file to exercise syntax highlighting, indents,
# and folds in vorto. Open with `vorto assets/samples/hello.hcl`.

variable "greeting" {
  type    = string
  default = "Hello"
}

locals {
  people = [
    { name = "Alice", age = 30 },
    { name = "Bob", age = 17 },
  ]

  adults = [for p in local.people : p.name if p.age >= 18]
}

resource "demo_person" "alice" {
  name = "Alice"
  age  = 30
  tags = ["admin", "early_bird"]

  settings {
    enabled = true
    ratio   = 0.75
  }
}

output "greeting_line" {
  value = "${var.greeting}, ${demo_person.alice.name}!"
}

output "adults" {
  value       = local.adults
  description = <<-EOT
    The names of all adult people,
    rendered as a heredoc string.
  EOT
}
