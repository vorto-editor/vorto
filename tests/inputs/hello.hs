-- Sample Haskell file to exercise syntax highlighting, indents,
-- and textobjects in vorto. Open with `vorto assets/samples/hello.hs`.

{-# LANGUAGE OverloadedStrings #-}

module Main where

import Data.List (foldl')
import qualified Data.Map.Strict as Map

-- A record with field accessors.
data Person = Person
  { name :: String
  , age  :: Int
  } deriving (Show, Eq)

-- A simple sum type with parameters.
data Tree a
  = Leaf
  | Node a (Tree a) (Tree a)
  deriving (Show)

-- Class + instance.
class Greet a where
  greet :: a -> String

instance Greet Person where
  greet p = "Hello, " ++ name p ++ "!"

-- Higher-order function, where clause, lambda, guards.
classify :: Int -> String
classify n
  | n < 0     = "negative"
  | n == 0    = "zero"
  | even n    = "positive even"
  | otherwise = "positive odd"
  where
    _ = "unused binding"

-- Pattern matching + recursion.
insert :: Ord a => a -> Tree a -> Tree a
insert x Leaf = Node x Leaf Leaf
insert x (Node y l r)
  | x < y     = Node y (insert x l) r
  | x > y     = Node y l (insert x r)
  | otherwise = Node y l r

-- IO with do-notation, string interpolation via show.
main :: IO ()
main = do
  let alice = Person { name = "Alice", age = 30 }
      nums  = [1 .. 10]
      tree  = foldl' (flip insert) Leaf nums
      table = Map.fromList [("one", 1), ("two", 2)]
  putStrLn (greet alice)
  mapM_ (\n -> putStrLn (show n ++ " is " ++ classify n)) nums
  print tree
  print table
