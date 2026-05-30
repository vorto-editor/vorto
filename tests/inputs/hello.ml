(* Sample OCaml file to exercise syntax highlighting, indents,
   and textobjects in vorto. Open with `vorto assets/samples/hello.ml`. *)

(** Tiny program exercising typical OCaml constructs:
    records, variants, modules, functors, pattern matching, and
    higher-order functions. *)

type person = {
  name : string;
  age : int;
  tags : string list;
}

type 'a tree =
  | Leaf
  | Node of 'a * 'a tree * 'a tree

module type GREETER = sig
  type subject
  val greet : subject -> string
end

module Person_greeter (P : sig val prefix : string end) :
  GREETER with type subject = person = struct
  type subject = person
  let greet p = Printf.sprintf "%s, %s!" P.prefix p.name
end

module Hi = Person_greeter (struct let prefix = "Hi" end)

let classify n =
  if n < 0 then "negative"
  else if n = 0 then "zero"
  else if n mod 2 = 0 then "positive even"
  else "positive odd"

let rec insert x = function
  | Leaf -> Node (x, Leaf, Leaf)
  | Node (y, l, r) ->
      if x < y then Node (y, insert x l, r)
      else if x > y then Node (y, l, insert x r)
      else Node (y, l, r)

let rec inorder = function
  | Leaf -> []
  | Node (x, l, r) -> inorder l @ [x] @ inorder r

let () =
  let people = [
    { name = "Alice"; age = 30; tags = ["admin"] };
    { name = "Bob"; age = 17; tags = [] };
    { name = "Carol"; age = 42; tags = ["vip"; "early-bird"] };
  ] in
  List.iter (fun p -> print_endline (Hi.greet p)) people;
  let adults = List.filter (fun p -> p.age >= 18) people in
  let names = List.map (fun p -> p.name) adults in
  Printf.printf "Adults: %s\n" (String.concat ", " names);
  for n = -2 to 5 do
    Printf.printf "%d is %s\n" n (classify n)
  done;
  let tree = List.fold_left (fun acc x -> insert x acc) Leaf [3; 1; 4; 1; 5; 9; 2; 6] in
  let sorted = inorder tree |> List.map string_of_int |> String.concat ", " in
  Printf.printf "sorted: %s\n" sorted
