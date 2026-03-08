# JavaScript Maps: A Comprehensive Guide

## Table of Contents
1. [What are JavaScript Maps?](#what-are-javascript-maps)
2. [Key Characteristics](#key-characteristics)
   - [Key-Value Pairs](#key-value-pairs)
   - [Insertion Order Preserved](#insertion-order-preserved)
   - [Any Type as Key](#any-type-as-key)
3. [Creating Maps](#creating-maps)
4. [Map vs Object: Detailed Comparison](#map-vs-object-detailed-comparison)
5. [Common Use Cases with Examples](#common-use-cases-with-examples)
6. [Map Methods Overview](#map-methods-overview)
7. [WeakMap](#weakmap)

---

## What are JavaScript Maps?

A JavaScript **Map** is a collection of keyed data items, similar to an Object. However, unlike Objects, Maps allow **keys of any type**, maintain **insertion order**, and provide better performance for frequent additions and removals. Maps were introduced in ES6 (ES2015) as a more versatile alternative to plain objects for certain use cases.

```javascript
// Creating a simple Map
const userMap = new Map();

userMap.set("id", 1);
userMap.set("name", "Alice");
userMap.set("isActive", true);

console.log(userMap.get("name")); // "Alice"
console.log(userMap.size);        // 3
```

**Key Differences from Objects at a glance:**
- Maps remember the original insertion order of the keys
- Any value (objects, functions, primitives) can be used as a key
- Maps have a `size` property; Objects require `Object.keys().length`
- Maps are iterable by default; Objects require `Object.*` methods
- Maps perform better for frequent additions and removals

---

## Key Characteristics

### 1. Key-Value Pairs

Like Objects, Maps store data as key-value pairs. However, Maps provide a more consistent API with dedicated `get()`, `set()`, `has()`, and `delete()` methods.

```javascript
const map = new Map();

// Setting values with set()
map.set("name", "John");
map.set(42, "The Answer");
map.set(true, "Yes");

// Getting values with get()
console.log(map.get("name"));  // "John"
console.log(map.get(42));      // "The Answer"
console.log(map.get(true));    // "Yes"

// Checking existence with has()
console.log(map.has("name"));  // true
console.log(map.has("age"));   // false

// Getting size
console.log(map.size);         // 3

// Chaining set() calls (returns the Map)
const chainedMap = new Map()
  .set("a", 1)
  .set("b", 2)
  .set("c", 3);

// Storing complex values
const user = { name: "Alice" };
const greeting = () => "Hello!";

map.set(user, "User Object");
map.set(greeting, "Function");

console.log(map.get(user));      // "User Object"
console.log(map.get(greeting));  // "Function"
```

**Key Points:**
- `set(key, value)` adds or updates a key-value pair
- `get(key)` retrieves a value; returns `undefined` if key doesn't exist
- `has(key)` checks for key existence (more reliable than `get()` for falsy values)
- `size` property gives the number of entries (faster than counting Object keys)

---

### 2. Insertion Order Preserved

**Maps guarantee that keys are iterated in their insertion order.** This is a significant advantage over Objects, where property order is not guaranteed by the specification.

```javascript
const insertionOrderMap = new Map();

// Adding keys in a specific order
insertionOrderMap.set("first", 1);
insertionOrderMap.set("second", 2);
insertionOrderMap.set("third", 3);
insertionOrderMap.set("fourth", 4);

// Iteration preserves insertion order
for (const [key, value] of insertionOrderMap) {
  console.log(`${key}: ${value}`);
}
// Output:
// first: 1
// second: 2
// third: 3
// fourth: 4

// Order is maintained even with numeric keys
const numericMap = new Map();
numericMap.set(10, "ten");
numericMap.set(1, "one");
numericMap.set(5, "five");
numericMap.set(100, "hundred");

console.log([...numericMap.keys()]);
// [10, 1, 5, 100] - insertion order preserved!

// Compare with Object behavior
const numericObj = {};
numericObj[10] = "ten";
numericObj[1] = "one";
numericObj[5] = "five";
numericObj[100] = "hundred";

console.log(Object.keys(numericObj));
// ["1", "5", "10", "100"] - sorted as strings!

// Order maintained after deletions and additions
insertionOrderMap.delete("second");
insertionOrderMap.set("fifth", 5);

console.log([...insertionOrderMap.keys()]);
// ["first", "third", "fourth", "fifth"] - still in order!
```

**Key Points:**
- Insertion order is **guaranteed** by the Map specification
- Works consistently with all key types (strings, numbers, objects, etc.)
- Deleting and re-adding a key puts it at the end
- No special handling for integer-like keys (unlike Objects)

---

### 3. Any Type as Key

**Maps accept any value as a key** — objects, functions, NaN, and even other Maps. This is a fundamental difference from Objects, which only accept strings and Symbols as keys.

```javascript
const anyKeyMap = new Map();

// Object keys
const obj1 = { id: 1 };
const obj2 = { id: 2 };
anyKeyMap.set(obj1, "First Object");
anyKeyMap.set(obj2, "Second Object");

console.log(anyKeyMap.get(obj1)); // "First Object"
console.log(anyKeyMap.get(obj2)); // "Second Object"

// Function keys
const func1 = () => "Function 1";
const func2 = () => "Function 2";
anyKeyMap.set(func1, "First Function");
anyKeyMap.set(func2, "Second Function");

console.log(anyKeyMap.get(func1)); // "First Function"

// Array keys
const arr1 = [1, 2, 3];
const arr2 = [4, 5, 6];
anyKeyMap.set(arr1, "Array 1-2-3");
anyKeyMap.set(arr2, "Array 4-5-6");

console.log(anyKeyMap.get(arr1)); // "Array 1-2-3"

// NaN as key (special case - NaN !== NaN, but Map handles it)
anyKeyMap.set(NaN, "Not a Number");
console.log(anyKeyMap.get(NaN));  // "Not a Number"
console.log(NaN === NaN);         // false (but Map finds it!)

// null and undefined as keys
anyKeyMap.set(null, "Null Value");
anyKeyMap.set(undefined, "Undefined Value");

console.log(anyKeyMap.get(null));      // "Null Value"
console.log(anyKeyMap.get(undefined)); // "Undefined Value"

// Using different object references
const keyObj = { name: "Key" };
anyKeyMap.set(keyObj, "Original");

const sameContent = { name: "Key" };
console.log(anyKeyMap.get(sameContent)); // undefined (different reference!)
console.log(anyKeyMap.get(keyObj));      // "Original"

// Demonstrating the SameValueZero comparison
const zeroMap = new Map();
zeroMap.set(+0, "Positive Zero");
console.log(zeroMap.get(-0)); // "Positive Zero" (+0 and -0 are considered equal)
```

**Key Points:**
- Keys are compared using **SameValueZero** algorithm (similar to `===` but treats NaN equal to NaN)
- Object keys use reference equality, not content equality
- `NaN` can be used as a key and retrieved correctly
- `+0` and `-0` are considered the same key

---

## Creating Maps

```javascript
// 1. Empty Map
const emptyMap = new Map();

// 2. From array of key-value pairs (most common)
const arrayMap = new Map([
  ["name", "Alice"],
  ["age", 25],
  ["city", "New York"]
]);

// 3. From another Map (creates a copy)
const original = new Map([["a", 1], ["b", 2]]);
const copy = new Map(original);
console.log(copy.get("a")); // 1

// 4. From Object.entries()
const obj = { x: 10, y: 20, z: 30 };
const fromObject = new Map(Object.entries(obj));
// Map(3) { 'x' => 10, 'y' => 20, 'z' => 30 }

// 5. From another iterable
const set = new Set([[1, "one"], [2, "two"]]);
const fromSet = new Map(set);

// 6. Using generator function
function* createEntries() {
  yield ["a", 1];
  yield ["b", 2];
  yield ["c", 3];
}
const fromGenerator = new Map(createEntries());

// 7. Clone a Map (shallow copy)
const originalMap = new Map([["key", { nested: "value" }]]);
const clonedMap = new Map(originalMap);
// Both maps reference the same nested object
```

---

## Map vs Object: Detailed Comparison

### Comparison Table

| Feature | Map | Object |
|---------|-----|--------|
| **Key Types** | Any type (objects, functions, primitives) | Strings and Symbols only |
| **Key Order** | Guaranteed insertion order | Not guaranteed (ES spec); insertion order in practice for string keys |
| **Size** | `size` property | `Object.keys(obj).length` |
| **Default Keys** | No default keys | Has prototype with default properties (`toString`, etc.) |
| **Iteration** | Directly iterable (`for...of`) | Requires `Object.keys/values/entries()` |
| **Performance** | Better for frequent additions/removals | Better for fixed-size, frequent reads |
| **Serialization** | No native JSON support | Native JSON support (`JSON.stringify`) |
| **Key Checking** | `has()` method | `in` operator or `hasOwnProperty()` |
| **Key Counting** | Direct `.size` property | Manual counting |

### Code Examples: Key Differences

```javascript
// 1. KEY TYPES
// =================

const map = new Map();
const obj = {};

// Map accepts any key type
map.set({}, "object key");
map.set(() => {}, "function key");
map.set(123, "number key");

// Object converts keys to strings
obj[{}] = "object key";       // Key becomes "[object Object]"
obj[() => {}] = "function";   // Key becomes function's toString()
obj[123] = "number key";      // Key becomes "123"

console.log(map.size);  // 3
console.log(Object.keys(obj).length);  // 3

// 2. INSERTION ORDER
// =================

const orderMap = new Map();
orderMap.set("z", 1);
orderMap.set("a", 2);
orderMap.set("m", 3);

const orderObj = {};
orderObj["z"] = 1;
orderObj["a"] = 2;
orderObj["m"] = 3;

console.log([...orderMap.keys()]);     // ["z", "a", "m"] - preserved!
console.log(Object.keys(orderObj));    // ["a", "m", "z"] - sorted!

// 3. DEFAULT KEYS / PROTOTYPE
// =================

const emptyMap = new Map();
const emptyObj = {};

console.log(emptyMap.has("toString"));  // false
console.log("toString" in emptyObj);    // true (inherited!)

// This can cause bugs with user input
const userInput = { toString: "user value" };
// obj[userInput] might accidentally access Object.prototype.toString

// 4. SIZE PROPERTY
// =================

const sizeMap = new Map([["a", 1], ["b", 2], ["c", 3]]);
const sizeObj = { a: 1, b: 2, c: 3 };

console.log(sizeMap.size);                         // 3 (O(1))
console.log(Object.keys(sizeObj).length);          // 3 (O(n))

// 5. ITERATION
// =================

const iterMap = new Map([["x", 1], ["y", 2]]);
const iterObj = { x: 1, y: 2 };

// Map - direct iteration
for (const [key, value] of iterMap) {
  console.log(key, value);
}

// Object - need conversion
for (const [key, value] of Object.entries(iterObj)) {
  console.log(key, value);
}

// Map has forEach with value, key order
iterMap.forEach((value, key) => {
  console.log(`${key} = ${value}`);
});

// 6. PERFORMANCE
// =================

// Benchmark scenario: frequent additions/removals
function benchmarkMap(count) {
  const map = new Map();
  console.time("Map");
  for (let i = 0; i < count; i++) {
    map.set(i, i);
  }
  for (let i = 0; i < count; i++) {
    map.delete(i);
  }
  console.timeEnd("Map");
}

function benchmarkObject(count) {
  const obj = {};
  console.time("Object");
  for (let i = 0; i < count; i++) {
    obj[i] = i;
  }
  for (let i = 0; i < count; i++) {
    delete obj[i];
  }
  console.timeEnd("Object");
}

// Maps generally perform better for large-scale add/remove operations

// 7. SERIALIZATION
// =================

const jsonMap = new Map([["name", "Alice"], ["data", { nested: true }]]);
const jsonObj = { name: "Alice", data: { nested: true } };

// Object serializes natively
console.log(JSON.stringify(jsonObj)); 
// {"name":"Alice","data":{"nested":true}}

// Map requires conversion
const mapAsObj = Object.fromEntries(jsonMap);
console.log(JSON.stringify(mapAsObj));
// {"name":"Alice","data":{"nested":true}}

// 8. KEY EXISTENCE CHECKING
// =================

const checkMap = new Map();
checkMap.set("key", undefined);

const checkObj = { key: undefined };

// Map - clear distinction
console.log(checkMap.has("key"));    // true (key exists)
console.log(checkMap.get("key"));    // undefined (value is undefined)

// Object - ambiguous
console.log("key" in checkObj);               // true
console.log(checkObj.hasOwnProperty("key"));  // true
console.log(checkObj.key !== undefined);      // false (problematic!)

// 9. CLEARING ALL ENTRIES
// =================

const clearMap = new Map([["a", 1], ["b", 2]]);
clearMap.clear();
console.log(clearMap.size);  // 0

const clearObj = { a: 1, b: 2 };
// No native clear; must iterate and delete
for (const key in clearObj) {
  if (clearObj.hasOwnProperty(key)) {
    delete clearObj[key];
  }
}
// Or reassign to new object (but breaks references)
```

### When to Use Map vs Object

**Use Map when:**
- Keys are unknown until runtime (user input, API responses)
- Keys are not strings or Symbols (objects, functions, DOM elements)
- You need to preserve insertion order
- You frequently add/remove entries
- You need to easily determine the size
- You want to avoid prototype chain issues
- You need keys that are object references

**Use Object when:**
- You need JSON serialization
- You're creating a simple structure with known string keys
- You're using the object as a namespace or container for methods
- Performance for small, fixed-size collections is critical
- You want property shorthand syntax
- You need property descriptors (getters, setters, non-enumerable properties)

---

## Common Use Cases with Examples

### 1. Caching with Object Keys

```javascript
// Cache function results using object arguments as keys
const computeCache = new Map();

function expensiveComputation(config) {
  // Check cache using the object reference
  if (computeCache.has(config)) {
    console.log("Cache hit!");
    return computeCache.get(config);
  }
  
  console.log("Computing...");
  // Simulate expensive operation
  const result = {
    data: `Processed ${JSON.stringify(config)}`,
    timestamp: Date.now()
  };
  
  computeCache.set(config, result);
  return result;
}

const config1 = { width: 100, height: 200, format: "png" };
const config2 = { width: 100, height: 200, format: "png" };

expensiveComputation(config1); // Computing...
expensiveComputation(config1); // Cache hit!
expensiveComputation(config2); // Computing... (different reference!)
```

### 2. Tracking DOM Elements

```javascript
// Associate data with DOM elements without modifying them
const elementData = new Map();

function attachData(element, data) {
  elementData.set(element, data);
}

function getData(element) {
  return elementData.get(element);
}

function removeData(element) {
  elementData.delete(element);
}

// Usage
const button = document.querySelector("#submit-btn");
attachData(button, {
  clickCount: 0,
  originalText: button.textContent,
  formId: "signup-form"
});

button.addEventListener("click", () => {
  const data = getData(button);
  data.clickCount++;
  console.log(`Clicked ${data.clickCount} times`);
});

// Cleanup when element is removed
removeData(button);
```

### 3. Counting Occurrences

```javascript
// Count frequency of items using any type as key
const inventory = new Map();

function addItem(item) {
  const current = inventory.get(item) || 0;
  inventory.set(item, current + 1);
}

function getCount(item) {
  return inventory.get(item) || 0;
}

// Can use objects as keys
const apple = { type: "fruit", name: "Apple" };
const banana = { type: "fruit", name: "Banana" };
const carrot = { type: "vegetable", name: "Carrot" };

addItem(apple);
addItem(apple);
addItem(banana);
addItem(carrot);
addItem(apple);

console.log(getCount(apple));   // 3
console.log(getCount(banana));  // 1
console.log(getCount(carrot));  // 1

// Iterate in insertion order
for (const [item, count] of inventory) {
  console.log(`${item.name}: ${count}`);
}
```

### 4. Bidirectional Mapping

```javascript
// Create two-way lookups between any types
class BiDirectionalMap {
  constructor() {
    this.keyToValue = new Map();
    this.valueToKey = new Map();
  }
  
  set(key, value) {
    this.keyToValue.set(key, value);
    this.valueToKey.set(value, key);
    return this;
  }
  
  getByKey(key) {
    return this.keyToValue.get(key);
  }
  
  getByValue(value) {
    return this.valueToKey.get(value);
  }
  
  hasKey(key) {
    return this.keyToValue.has(key);
  }
  
  hasValue(value) {
    return this.valueToKey.has(value);
  }
  
  deleteByKey(key) {
    const value = this.keyToValue.get(key);
    this.keyToValue.delete(key);
    this.valueToKey.delete(value);
    return this;
  }
  
  get size() {
    return this.keyToValue.size;
  }
}

// Usage with mixed types
const statusMap = new BiDirectionalMap();

const pendingSymbol = Symbol("PENDING");
const activeObj = { status: "active" };

statusMap.set(pendingSymbol, "Pending");
statusMap.set(activeObj, "Active");
statusMap.set(200, "OK");

console.log(statusMap.getByKey(pendingSymbol));  // "Pending"
console.log(statusMap.getByValue("Active"));     // { status: "active" }
console.log(statusMap.getByKey(200));            // "OK"
```

### 5. Memoization with Multiple Arguments

```javascript
// Advanced memoization supporting object/function arguments
const memoize = (fn) => {
  const cache = new Map();
  
  return function(...args) {
    // Use args array as key (Map handles object references)
    const key = args;
    
    // Find matching arguments (deep comparison could be added)
    for (const [cachedArgs, result] of cache) {
      if (args.length === cachedArgs.length && 
          args.every((arg, i) => arg === cachedArgs[i])) {
        console.log("Memoized result");
        return result;
      }
    }
    
    const result = fn.apply(this, args);
    cache.set(key, result);
    return result;
  };
};

const calculateDistance = memoize((point1, point2) => {
  console.log("Calculating...");
  const dx = point2.x - point1.x;
  const dy = point2.y - point1.y;
  return Math.sqrt(dx * dx + dy * dy);
});

const p1 = { x: 0, y: 0 };
const p2 = { x: 3, y: 4 };
const p3 = { x: 0, y: 0 };
const p4 = { x: 3, y: 4 };

calculateDistance(p1, p2); // Calculating... → 5
calculateDistance(p1, p2); // Memoized result → 5
calculateDistance(p3, p4); // Calculating... → 5 (different references)
```

### 6. State Management

```javascript
// Simple state store using Map
class StateStore {
  constructor() {
    this.state = new Map();
    this.listeners = new Map();
  }
  
  set(key, value) {
    const oldValue = this.state.get(key);
    this.state.set(key, value);
    this.notify(key, value, oldValue);
  }
  
  get(key) {
    return this.state.get(key);
  }
  
  subscribe(key, callback) {
    if (!this.listeners.has(key)) {
      this.listeners.set(key, new Set());
    }
    this.listeners.get(key).add(callback);
    
    // Return unsubscribe function
    return () => {
      this.listeners.get(key).delete(callback);
    };
  }
  
  notify(key, newValue, oldValue) {
    const keyListeners = this.listeners.get(key);
    if (keyListeners) {
      keyListeners.forEach(callback => {
        callback(newValue, oldValue);
      });
    }
  }
}

// Usage
const store = new StateStore();

const unsubscribe = store.subscribe("user", (newVal, oldVal) => {
  console.log(`User changed from ${oldVal?.name} to ${newVal.name}`);
});

store.set("user", { name: "Alice", id: 1 });
store.set("user", { name: "Bob", id: 2 });

unsubscribe();
```

### 7. Grouping Data

```javascript
// Group items by any key type
function groupBy(items, keySelector) {
  const groups = new Map();
  
  for (const item of items) {
    const key = keySelector(item);
    if (!groups.has(key)) {
      groups.set(key, []);
    }
    groups.get(key).push(item);
  }
  
  return groups;
}

const employees = [
  { name: "Alice", department: "Engineering", level: "Senior" },
  { name: "Bob", department: "Engineering", level: "Junior" },
  { name: "Charlie", department: "Sales", level: "Senior" },
  { name: "Diana", department: "Sales", level: "Junior" },
  { name: "Eve", department: "Engineering", level: "Senior" }
];

// Group by department (string)
const byDept = groupBy(employees, e => e.department);
for (const [dept, people] of byDept) {
  console.log(`${dept}: ${people.map(p => p.name).join(", ")}`);
}

// Group by level (string)
const byLevel = groupBy(employees, e => e.level);

// Group by custom criteria (object key)
const seniorityKey = { isSenior: true };
const juniorKey = { isSenior: false };

const byCustom = groupBy(employees, e => 
  e.level === "Senior" ? seniorityKey : juniorKey
);
```

### 8. Rate Limiting / Throttling Tracking

```javascript
// Track API call counts per user
class RateLimiter {
  constructor(maxRequests, windowMs) {
    this.maxRequests = maxRequests;
    this.windowMs = windowMs;
    this.requests = new Map(); // user -> [{timestamp}]
  }
  
  canProceed(userId) {
    const now = Date.now();
    const userRequests = this.requests.get(userId) || [];
    
    // Remove old requests outside the window
    const validRequests = userRequests.filter(
      time => now - time < this.windowMs
    );
    
    if (validRequests.length < this.maxRequests) {
      validRequests.push(now);
      this.requests.set(userId, validRequests);
      return { allowed: true, remaining: this.maxRequests - validRequests.length };
    }
    
    const retryAfter = Math.ceil(
      (validRequests[0] + this.windowMs - now) / 1000
    );
    
    return { allowed: false, retryAfter };
  }
}

// Usage
const limiter = new RateLimiter(5, 60000); // 5 requests per minute

const user1 = { id: 1, name: "Alice" };
const user2 = { id: 2, name: "Bob" };

// Objects can be used as keys directly
console.log(limiter.canProceed(user1)); // { allowed: true, remaining: 4 }
console.log(limiter.canProceed(user1)); // { allowed: true, remaining: 3 }
console.log(limiter.canProceed(user2)); // { allowed: true, remaining: 4 }
```

---

## Map Methods Overview

### Instance Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `set(key, value)` | Adds/updates key-value pair | Map (for chaining) |
| `get(key)` | Retrieves value by key | Value or `undefined` |
| `has(key)` | Checks if key exists | Boolean |
| `delete(key)` | Removes entry by key | Boolean (success) |
| `clear()` | Removes all entries | `undefined` |
| `forEach(callback)` | Iterates over entries | `undefined` |
| `keys()` | Returns iterator of keys | Map Iterator |
| `values()` | Returns iterator of values | Map Iterator |
| `entries()` | Returns iterator of [key, value] pairs | Map Iterator |
| `[Symbol.iterator]()` | Makes Map iterable | Same as `entries()` |

### Property

| Property | Description |
|----------|-------------|
| `size` | Number of entries in the Map |

### Code Examples

```javascript
const map = new Map([
  ["a", 1],
  ["b", 2],
  ["c", 3]
]);

// Iteration methods
console.log([...map.keys()]);    // ["a", "b", "c"]
console.log([...map.values()]);  // [1, 2, 3]
console.log([...map.entries()]); // [["a", 1], ["b", 2], ["c", 3]]

// forEach (value, key, map)
map.forEach((value, key) => {
  console.log(`${key}: ${value}`);
});

// Destructuring in for...of
for (const [key, value] of map) {
  console.log(`${key} = ${value}`);
}

// Chaining
const result = new Map()
  .set("x", 10)
  .set("y", 20)
  .set("z", 30);

// Conversion to other formats
const asObject = Object.fromEntries(map);
// { a: 1, b: 2, c: 3 }

const asArray = Array.from(map);
// [["a", 1], ["b", 2], ["c", 3]]

const keysArray = [...map.keys()];
// ["a", "b", "c"]
```

---

## WeakMap

**WeakMap** is a variant of Map that holds "weak" references to its keys. This means:
- Keys must be objects (not primitives)
- If no other references to a key exist, it can be garbage collected
- Not enumerable (no `size`, `keys()`, `values()`, `entries()`, `forEach()`)
- Useful for private data and metadata association

```javascript
// WeakMap for private data
const privateData = new WeakMap();

class User {
  constructor(name, password) {
    privateData.set(this, { password, createdAt: new Date() });
    this.name = name;
  }
  
  getCreatedAt() {
    return privateData.get(this).createdAt;
  }
  
  verifyPassword(pwd) {
    return privateData.get(this).password === pwd;
  }
}

const user = new User("Alice", "secret123");
console.log(user.name);              // "Alice"
console.log(user.password);          // undefined (private!)
console.log(user.verifyPassword("secret123")); // true

// WeakMap for DOM metadata
const clickCounts = new WeakMap();

function trackClicks(element) {
  const current = clickCounts.get(element) || 0;
  clickCounts.set(element, current + 1);
  console.log(`Element clicked ${current + 1} times`);
}

const myButton = document.querySelector("#myButton");
myButton.addEventListener("click", () => trackClicks(myButton));

// When myButton is removed from DOM and has no other references,
// the entry in clickCounts is automatically garbage collected
```

### Map vs WeakMap

| Feature | Map | WeakMap |
|---------|-----|---------|
| **Key Types** | Any | Objects only |
| **References** | Strong | Weak (allows GC) |
| **Enumerable** | Yes | No |
| **Size Property** | Yes | No |
| **Iteration** | Full support | Not iterable |
| **Use Case** | General data storage | Private data, metadata |

---

## Summary

| Characteristic | Description |
|----------------|-------------|
| **Key-Value Pairs** | Store data with get/set/has/delete API |
| **Insertion Order** | Guaranteed order of iteration |
| **Any Type as Key** | Objects, functions, primitives all valid |
| **Performance** | Optimized for frequent additions/removals |
| **No Default Keys** | Clean slate, no prototype chain issues |
| **Directly Iterable** | Works with `for...of` without conversion |
| **Size Property** | O(1) access to entry count |

Maps provide a powerful, flexible alternative to plain Objects when you need ordered iteration, non-string keys, or frequent modifications. Use Maps for dynamic data structures and Objects for simple, static structures with string keys.
