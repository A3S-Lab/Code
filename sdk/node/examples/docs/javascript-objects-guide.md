# JavaScript Objects: A Comprehensive Guide

## Table of Contents
1. [What are JavaScript Objects?](#what-are-javascript-objects)
2. [Key Characteristics](#key-characteristics)
   - [Key-Value Pairs](#key-value-pairs)
   - [Unordered Collections](#unordered-collections)
   - [String and Symbol Keys](#string-and-symbol-keys)
3. [Creating Objects](#creating-objects)
4. [Accessing and Modifying Properties](#accessing-and-modifying-properties)
5. [Common Use Cases with Examples](#common-use-cases-with-examples)
6. [Object Methods Overview](#object-methods-overview)
7. [Prototypes and Inheritance](#prototypes-and-inheritance)

---

## What are JavaScript Objects?

A JavaScript **Object** is a collection of related data and functionality stored as **key-value pairs**. Objects are the foundation of JavaScript — nearly everything in JavaScript is an object or behaves like one. They allow you to group related data and functions together into a single entity.

```javascript
// A simple object representing a person
const person = {
  firstName: "John",
  lastName: "Doe",
  age: 30,
  isEmployed: true,
  greet: function() {
    return `Hello, I'm ${this.firstName} ${this.lastName}`;
  }
};

console.log(person.greet()); // "Hello, I'm John Doe"
```

---

## Key Characteristics

### 1. Key-Value Pairs

Objects store data as **properties**, where each property consists of a **key** (also called property name) and a **value**. This structure makes objects perfect for representing real-world entities and their attributes.

```javascript
const car = {
  brand: "Toyota",      // key: "brand", value: "Toyota"
  model: "Camry",       // key: "model", value: "Camry"
  year: 2023,           // key: "year", value: 2023
  color: "blue",        // key: "color", value: "blue"
  mileage: 15000        // key: "mileage", value: 15000
};

// Keys are strings (or Symbols) that identify the value
// Values can be any JavaScript data type
const complexObject = {
  name: "Complex",                    // string value
  count: 42,                          // number value
  isActive: true,                     // boolean value
  items: [1, 2, 3],                   // array value
  nested: { a: 1, b: 2 },             // object value
  doSomething: function() { },        // function value (method)
  createdAt: new Date()               // object instance
};

// Computed property names (dynamic keys)
const propName = "dynamicKey";
const obj = {
  [propName]: "This key is dynamic",
  [`computed_${1 + 1}`]: "Expression result as key"
};
console.log(obj); // { dynamicKey: "...", computed_2: "..." }
```

**Key Points:**
- Keys are unique within an object (duplicate keys overwrite previous values)
- Values can be primitive types, objects, arrays, or functions
- ES6+ supports computed property names using bracket notation in object literals
- Keys are automatically converted to strings (except Symbols)

---

### 2. Unordered Collections

Unlike arrays, **objects do not guarantee any specific order** of their properties. While modern JavaScript engines typically maintain insertion order for string keys in practice, this is not guaranteed by the specification.

```javascript
const settings = {
  theme: "dark",
  notifications: true,
  language: "en",
  autoSave: false
};

// Properties may not iterate in insertion order
console.log(Object.keys(settings));
// Could be: ["theme", "notifications", "language", "autoSave"]
// Or any other order - not guaranteed!

// The order is less predictable with mixed key types
const mixed = {
  "1": "one",
  "name": "John",
  "10": "ten",
  "age": 30
};

// Numeric-like keys may be sorted
console.log(Object.keys(mixed));
// Typically: ["1", "10", "name", "age"] (numeric keys sorted first)

// For guaranteed order, use Map or Array
const ordered = new Map([
  ["first", 1],
  ["second", 2],
  ["third", 3]
]);
// Maps guarantee insertion order
```

**Key Points:**
- Do not rely on property order for program logic
- Integer-like keys (e.g., "1", "10") may be sorted numerically
- Use `Map` when insertion order is critical
- Use arrays when order matters and you need indexed access

---

### 3. String and Symbol Keys

Object keys can only be **strings** or **Symbols**. Other types (numbers, objects) are automatically converted to strings.

```javascript
// String keys (most common)
const stringKeys = {
  name: "Alice",
  "full-name": "Alice Smith",  // Quoted for special characters
  "123": "numeric string"      // Numbers become strings
};

// Symbol keys (unique, non-enumerable by default)
const id = Symbol("id");
const secret = Symbol("secret");

const symbolKeys = {
  name: "Product",
  [id]: 12345,           // Symbol as key (bracket notation required)
  [secret]: "hidden"     // Another Symbol key
};

console.log(symbolKeys.name);     // "Product" (accessible)
console.log(symbolKeys[id]);      // 12345 (accessible with reference)
console.log(symbolKeys.secret);   // undefined (not the same Symbol!)

// Symbols are unique
const anotherId = Symbol("id");
console.log(id === anotherId);    // false (different Symbols)

// Symbols don't appear in Object.keys()
console.log(Object.keys(symbolKeys)); // ["name"]

// But they do appear in Object.getOwnPropertySymbols()
console.log(Object.getOwnPropertySymbols(symbolKeys)); // [Symbol(id), Symbol(secret)]

// Number keys are converted to strings
const numberTest = {};
numberTest[42] = "forty-two";
console.log(numberTest["42"]);    // "forty-two"
console.log(Object.keys(numberTest)); // ["42"] (string!)

// Object keys are converted to strings too!
const objKey = { toString: () => "customKey" };
const testObj = {};
testObj[objKey] = "value";
console.log(testObj["customKey"]); // "value"
```

**Key Points:**
- String keys are the default and most common
- Symbol keys provide unique identifiers that won't conflict
- Non-string keys are coerced to strings using `toString()`
- Well-known Symbols (e.g., `Symbol.iterator`) define object behavior

---

## Creating Objects

```javascript
// 1. Object literal (most common)
const obj1 = { name: "Alice", age: 25 };

// 2. Object constructor (not recommended)
const obj2 = new Object();
obj2.name = "Bob";
obj2.age = 30;

// 3. Object.create() - specify prototype
const prototype = { greet() { return "Hello!"; } };
const obj3 = Object.create(prototype);
obj3.name = "Charlie";

// 4. Constructor function (ES5 style)
function Person(name, age) {
  this.name = name;
  this.age = age;
}
const obj4 = new Person("David", 35);

// 5. ES6 Class
class Employee {
  constructor(name, role) {
    this.name = name;
    this.role = role;
  }
  describe() {
    return `${this.name} is a ${this.role}`;
  }
}
const obj5 = new Employee("Eve", "Developer");

// 6. Factory function
function createUser(name, email) {
  return {
    name,
    email,
    createdAt: new Date()
  };
}
const obj6 = createUser("Frank", "frank@example.com");

// 7. Object.fromEntries() - from array of key-value pairs
const entries = [["a", 1], ["b", 2], ["c", 3]];
const obj7 = Object.fromEntries(entries);
// { a: 1, b: 2, c: 3 }

// 8. Spread operator (copy/merge)
const base = { x: 1, y: 2 };
const obj8 = { ...base, z: 3 };
// { x: 1, y: 2, z: 3 }
```

---

## Accessing and Modifying Properties

```javascript
const user = {
  firstName: "John",
  lastName: "Doe",
  "user-id": "jd123",
  address: {
    city: "New York",
    zip: "10001"
  }
};

// Dot notation (for valid identifiers)
console.log(user.firstName); // "John"
user.age = 30;               // Add new property

// Bracket notation (required for special characters, variables)
console.log(user["user-id"]); // "jd123"
const prop = "lastName";
console.log(user[prop]);      // "Doe"

// Optional chaining (safe nested access)
console.log(user.address?.city);      // "New York"
console.log(user.profile?.bio);       // undefined (no error!)

// Destructuring
const { firstName, lastName, age = 25 } = user;
// firstName = "John", lastName = "Doe", age = 30 (default not used)

// Nested destructuring
const { address: { city } } = user;
// city = "New York"

// Dynamic property access
const getProperty = (obj, key) => obj[key];

// Computed property assignment
const key = "email";
user[key] = "john@example.com";

// Deleting properties
delete user.age;

// Checking property existence
console.log("firstName" in user);     // true
console.log(user.hasOwnProperty("firstName")); // true
console.log(user.age !== undefined);   // false
```

---

## Common Use Cases with Examples

### 1. Data Modeling / Entity Representation

```javascript
// Representing real-world entities
const book = {
  title: "The Great Gatsby",
  author: {
    firstName: "F. Scott",
    lastName: "Fitzgerald"
  },
  published: 1925,
  genres: ["Fiction", "Classic"],
  getSummary() {
    return `${this.title} by ${this.author.firstName} ${this.author.lastName}`;
  },
  isClassic() {
    return new Date().getFullYear() - this.published > 50;
  }
};

console.log(book.getSummary()); // "The Great Gatsby by F. Scott Fitzgerald"
console.log(book.isClassic());  // true

// Collections of entities
const library = {
  books: [
    { id: 1, title: "Book 1", available: true },
    { id: 2, title: "Book 2", available: false },
    { id: 3, title: "Book 3", available: true }
  ],
  findById(id) {
    return this.books.find(book => book.id === id);
  },
  getAvailable() {
    return this.books.filter(book => book.available);
  }
};
```

### 2. Configuration Objects

```javascript
// Application settings
const config = {
  api: {
    baseUrl: "https://api.example.com",
    version: "v2",
    timeout: 5000,
    retries: 3
  },
  database: {
    host: "localhost",
    port: 5432,
    name: "myapp",
    ssl: true
  },
  features: {
    darkMode: true,
    notifications: true,
    betaAccess: false
  },
  getApiUrl(endpoint) {
    return `${this.api.baseUrl}/${this.api.version}/${endpoint}`;
  }
};

// Usage
const userEndpoint = config.getApiUrl("users");
// "https://api.example.com/v2/users"

// Default configurations with override
function initializeApp(userConfig = {}) {
  const finalConfig = {
    theme: "light",
    language: "en",
    debug: false,
    ...userConfig  // Override defaults
  };
  return finalConfig;
}

const myApp = initializeApp({ theme: "dark", debug: true });
// { theme: "dark", language: "en", debug: true }
```

### 3. Namespacing / Organizing Code

```javascript
// Organizing related functions into namespaces
const MathUtils = {
  PI: 3.14159,
  
  add(a, b) {
    return a + b;
  },
  
  subtract(a, b) {
    return a - b;
  },
  
  multiply(a, b) {
    return a * b;
  },
  
  divide(a, b) {
    if (b === 0) throw new Error("Division by zero");
    return a / b;
  },
  
  circleArea(radius) {
    return this.PI * radius * radius;
  }
};

console.log(MathUtils.add(5, 3));           // 8
console.log(MathUtils.circleArea(5));       // 78.53975

// API namespace
const API = {
  baseUrl: "/api",
  
  endpoints: {
    users: "/users",
    posts: "/posts",
    comments: "/comments"
  },
  
  async getUsers() {
    const response = await fetch(`${this.baseUrl}${this.endpoints.users}`);
    return response.json();
  },
  
  async getUserById(id) {
    const response = await fetch(`${this.baseUrl}${this.endpoints.users}/${id}`);
    return response.json();
  }
};
```

### 4. Lookup Tables / Dictionaries

```javascript
// Fast O(1) lookups using objects as hash maps
const statusCodes = {
  200: "OK",
  201: "Created",
  400: "Bad Request",
  401: "Unauthorized",
  403: "Forbidden",
  404: "Not Found",
  500: "Internal Server Error"
};

function getStatusMessage(code) {
  return statusCodes[code] || "Unknown Status";
}

console.log(getStatusMessage(200)); // "OK"
console.log(getStatusMessage(418)); // "Unknown Status"

// State management
const stateMachine = {
  IDLE: "idle",
  LOADING: "loading",
  SUCCESS: "success",
  ERROR: "error"
};

// Permission mapping
const permissions = {
  admin: ["read", "write", "delete", "manage"],
  editor: ["read", "write"],
  viewer: ["read"]
};

function hasPermission(role, action) {
  return permissions[role]?.includes(action) ?? false;
}

console.log(hasPermission("editor", "write"));  // true
console.log(hasPermission("viewer", "delete")); // false
```

### 5. Caching / Memoization

```javascript
// Simple memoization using objects
function createMemoizedFunction(fn) {
  const cache = {};
  
  return function(...args) {
    const key = JSON.stringify(args);
    
    if (key in cache) {
      console.log("Cache hit!");
      return cache[key];
    }
    
    console.log("Computing...");
    const result = fn.apply(this, args);
    cache[key] = result;
    return result;
  };
}

// Expensive computation
const fibonacci = createMemoizedFunction(function(n) {
  if (n < 2) return n;
  return this(n - 1) + this(n - 2);
}.bind(fibonacci));

console.log(fibonacci(40)); // Computing... 102334155
console.log(fibonacci(40)); // Cache hit!   102334155

// API response cache
const apiCache = {
  store: {},
  
  set(key, data, ttl = 60000) {
    this.store[key] = {
      data,
      expires: Date.now() + ttl
    };
  },
  
  get(key) {
    const entry = this.store[key];
    if (!entry) return null;
    if (Date.now() > entry.expires) {
      delete this.store[key];
      return null;
    }
    return entry.data;
  },
  
  clear() {
    this.store = {};
  }
};
```

### 6. Method Chaining (Fluent Interface)

```javascript
class QueryBuilder {
  constructor() {
    this.query = {
      select: [],
      from: "",
      where: [],
      orderBy: "",
      limit: null
    };
  }
  
  select(fields) {
    this.query.select = Array.isArray(fields) ? fields : [fields];
    return this; // Return this for chaining
  }
  
  from(table) {
    this.query.from = table;
    return this;
  }
  
  where(condition) {
    this.query.where.push(condition);
    return this;
  }
  
  orderBy(field, direction = "ASC") {
    this.query.orderBy = `${field} ${direction}`;
    return this;
  }
  
  limit(count) {
    this.query.limit = count;
    return this;
  }
  
  build() {
    let sql = `SELECT ${this.query.select.join(", ") || "*"}`;
    sql += ` FROM ${this.query.from}`;
    
    if (this.query.where.length) {
      sql += ` WHERE ${this.query.where.join(" AND ")}`;
    }
    
    if (this.query.orderBy) {
      sql += ` ORDER BY ${this.query.orderBy}`;
    }
    
    if (this.query.limit) {
      sql += ` LIMIT ${this.query.limit}`;
    }
    
    return sql;
  }
}

// Fluent method chaining
const query = new QueryBuilder()
  .select(["name", "email", "age"])
  .from("users")
  .where("age > 18")
  .where("active = true")
  .orderBy("created_at", "DESC")
  .limit(10)
  .build();

console.log(query);
// SELECT name, email, age FROM users WHERE age > 18 AND active = true ORDER BY created_at DESC LIMIT 10
```

### 7. JSON Serialization / Data Transfer

```javascript
// Converting objects to/from JSON
const user = {
  id: 1,
  name: "Alice Smith",
  email: "alice@example.com",
  preferences: {
    theme: "dark",
    notifications: true
  },
  createdAt: new Date()
};

// Serialize to JSON
const jsonString = JSON.stringify(user, null, 2);
console.log(jsonString);
/*
{
  "id": 1,
  "name": "Alice Smith",
  "email": "alice@example.com",
  "preferences": {
    "theme": "dark",
    "notifications": true
  },
  "createdAt": "2024-01-15T10:30:00.000Z"
}
*/

// Custom serialization
const customJSON = JSON.stringify(user, (key, value) => {
  // Hide sensitive data
  if (key === "email") return undefined;
  // Format dates
  if (value instanceof Date) return value.toISOString().split("T")[0];
  return value;
}, 2);

// Parse JSON back to object
const parsed = JSON.parse(jsonString);

// Reviver function for parsing
const parsedWithDate = JSON.parse(jsonString, (key, value) => {
  if (key === "createdAt") return new Date(value);
  return value;
});

console.log(parsedWithDate.createdAt instanceof Date); // true
```

### 8. Event Emitters / Pub-Sub Pattern

```javascript
// Simple event emitter using objects
class EventEmitter {
  constructor() {
    this.events = {};
  }
  
  on(event, listener) {
    if (!this.events[event]) {
      this.events[event] = [];
    }
    this.events[event].push(listener);
    return this; // For chaining
  }
  
  off(event, listenerToRemove) {
    if (!this.events[event]) return this;
    
    this.events[event] = this.events[event].filter(
      listener => listener !== listenerToRemove
    );
    return this;
  }
  
  emit(event, ...args) {
    if (!this.events[event]) return this;
    
    this.events[event].forEach(listener => {
      listener.apply(this, args);
    });
    return this;
  }
  
  once(event, listener) {
    const onceWrapper = (...args) => {
      listener.apply(this, args);
      this.off(event, onceWrapper);
    };
    this.on(event, onceWrapper);
    return this;
  }
}

// Usage
const emitter = new EventEmitter();

emitter
  .on("user:login", (user) => {
    console.log(`User ${user.name} logged in`);
  })
  .on("user:login", (user) => {
    console.log(`Sending welcome email to ${user.email}`);
  })
  .once("app:init", () => {
    console.log("App initialized (runs only once)");
  });

emitter.emit("user:login", { name: "Alice", email: "alice@example.com" });
// User Alice logged in
// Sending welcome email to alice@example.com

emitter.emit("app:init");
emitter.emit("app:init"); // Won't fire again
```

### 9. Factory Pattern / Object Creation

```javascript
// Factory functions for creating objects with shared methods
function createCounter(initialValue = 0) {
  let count = initialValue; // Private variable (closure)
  
  return {
    increment() {
      count++;
      return this;
    },
    decrement() {
      count--;
      return this;
    },
    getValue() {
      return count;
    },
    reset() {
      count = initialValue;
      return this;
    }
  };
}

const counter1 = createCounter(10);
const counter2 = createCounter();

counter1.increment().increment();
console.log(counter1.getValue()); // 12
console.log(counter2.getValue()); // 0

// Factory with options object pattern
function createComponent(options = {}) {
  const {
    tag = "div",
    className = "",
    id = "",
    text = "",
    onClick = null,
    children = []
  } = options;
  
  return {
    tag,
    className,
    id,
    text,
    onClick,
    children,
    
    render() {
      const element = document.createElement(this.tag);
      if (this.className) element.className = this.className;
      if (this.id) element.id = this.id;
      if (this.text) element.textContent = this.text;
      if (this.onClick) element.addEventListener("click", this.onClick);
      this.children.forEach(child => element.appendChild(child.render()));
      return element;
    }
  };
}

const button = createComponent({
  tag: "button",
  className: "btn btn-primary",
  text: "Click me",
  onClick: () => console.log("Clicked!")
});
```

### 10. Property Descriptors and Advanced Features

```javascript
const product = {};

// Define property with descriptor
Object.defineProperty(product, "name", {
  value: "Laptop",
  writable: true,
  enumerable: true,
  configurable: true
});

// Getter and setter
Object.defineProperty(product, "price", {
  get() {
    return this._price;
  },
  set(value) {
    if (value < 0) throw new Error("Price cannot be negative");
    this._price = value;
  },
  enumerable: true,
  configurable: true
});

// Multiple properties
Object.defineProperties(product, {
  stock: {
    value: 100,
    writable: true,
    enumerable: true
  },
  sku: {
    value: "LAPTOP-001",
    writable: false, // Read-only
    enumerable: true
  }
});

product.price = 999;
console.log(product.price); // 999
// product.sku = "NEW"; // Error: Cannot assign to read-only property

// Get property descriptor
const descriptor = Object.getOwnPropertyDescriptor(product, "price");
console.log(descriptor);
// { get: [Function: get], set: [Function: set], ... }

// Sealing and freezing
const config = { apiUrl: "https://api.example.com" };

Object.seal(config); // Prevent adding/removing properties
config.apiUrl = "https://new.api.com"; // OK
// config.newProp = "value"; // Error in strict mode

Object.freeze(config); // Make completely immutable
// config.apiUrl = "..."; // Error in strict mode

// Check state
console.log(Object.isSealed(config));   // true
console.log(Object.isFrozen(config));   // true
console.log(Object.isExtensible(config)); // false
```

---

## Object Methods Overview

### Static Methods (Object.*)

| Method | Purpose |
|--------|---------|
| `Object.keys(obj)` | Returns array of enumerable property names |
| `Object.values(obj)` | Returns array of property values |
| `Object.entries(obj)` | Returns array of [key, value] pairs |
| `Object.fromEntries(entries)` | Creates object from array of entries |
| `Object.assign(target, ...sources)` | Copies properties from sources to target |
| `Object.create(proto)` | Creates new object with specified prototype |
| `Object.defineProperty(obj, prop, descriptor)` | Adds/modifies property with descriptor |
| `Object.getOwnPropertyDescriptor(obj, prop)` | Returns property descriptor |
| `Object.getOwnPropertyNames(obj)` | Returns all property names (including non-enumerable) |
| `Object.getOwnPropertySymbols(obj)` | Returns all Symbol properties |
| `Object.freeze(obj)` | Makes object immutable |
| `Object.seal(obj)` | Prevents adding/removing properties |
| `Object.preventExtensions(obj)` | Prevents adding new properties |
| `Object.isFrozen(obj)` | Check if frozen |
| `Object.isSealed(obj)` | Check if sealed |
| `Object.isExtensible(obj)` | Check if can add properties |

### Instance Methods (obj.*)

| Method | Purpose |
|--------|---------|
| `obj.hasOwnProperty(prop)` | Check if property is own (not inherited) |
| `obj.isPrototypeOf(other)` | Check if obj is in other's prototype chain |
| `obj.propertyIsEnumerable(prop)` | Check if property is enumerable |
| `obj.toString()` | Returns string representation |
| `obj.valueOf()` | Returns primitive value |

---

## Prototypes and Inheritance

```javascript
// Prototype chain
const animal = {
  eats: true,
  walk() {
    console.log("Animal walks");
  }
};

const rabbit = {
  jumps: true,
  __proto__: animal // Set prototype (modern syntax)
};

// Or using Object.create
const dog = Object.create(animal);
dog.barks = true;
dog.walk = function() {
  console.log("Dog runs");
};

rabbit.walk(); // "Animal walks" (inherited)
dog.walk();    // "Dog runs" (overridden)

// Check prototype
console.log(Object.getPrototypeOf(rabbit) === animal); // true

// Constructor functions and prototype
function Vehicle(type) {
  this.type = type;
}

Vehicle.prototype.describe = function() {
  return `This is a ${this.type}`;
};

const car = new Vehicle("car");
console.log(car.describe()); // "This is a car"

// ES6 Classes (syntactic sugar over prototypes)
class Animal {
  constructor(name) {
    this.name = name;
  }
  
  speak() {
    return `${this.name} makes a sound`;
  }
}

class Dog extends Animal {
  speak() {
    return `${this.name} barks`;
  }
}

const myDog = new Dog("Rex");
console.log(myDog.speak()); // "Rex barks"
```

---

## Summary

| Characteristic | Description |
|----------------|-------------|
| **Key-Value Pairs** | Store data as named properties with associated values |
| **Unordered** | No guaranteed iteration order; use Map for ordered data |
| **String/Symbol Keys** | Keys must be strings or Symbols; other types are coerced |
| **Dynamic** | Properties can be added, modified, or deleted at any time |
| **Reference Type** | Objects are passed by reference, not by value |
| **Prototype-based** | Inherit properties and methods via prototype chain |

JavaScript objects are versatile data structures essential for organizing code, modeling data, managing state, and implementing various design patterns in JavaScript applications.
