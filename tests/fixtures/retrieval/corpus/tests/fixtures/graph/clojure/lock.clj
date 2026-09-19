(ns lock (:require [clojure.string :as str]))

(defn helper [] 1)

(defn acquire [] (helper))
