(ns lock (:require [clojure.string :as str]))

(def max-holders 4)

(defn helper [] 1)

(defn acquire [] (helper))
