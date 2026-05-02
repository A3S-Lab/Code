#!/usr/bin/env python3
"""
Test script for chart-generator skill

This script demonstrates how to use the chart-generator skill
to generate vis-chart formatted visualizations.
"""

from a3s_code import Agent, SessionOptions

def test_basic_chart():
    """Test basic chart generation"""
    print("=" * 60)
    print("Test 1: Basic Line Chart")
    print("=" * 60)

    agent = Agent.create("agent.acl")
    session = agent.session(".", builtin_skills=True)

    result = session.send("""
    Create a line chart showing quarterly sales:
    Q1: 120, Q2: 150, Q3: 180, Q4: 220
    Use vis-chart format.
    """)

    print(result.text)
    print()

def test_data_file_chart():
    """Test chart generation from data file"""
    print("=" * 60)
    print("Test 2: Chart from Data File")
    print("=" * 60)

    # Create sample data file
    import json
    data = {
        "products": [
            {"name": "Product A", "sales": 450},
            {"name": "Product B", "sales": 380},
            {"name": "Product C", "sales": 520},
            {"name": "Product D", "sales": 180}
        ]
    }

    with open("/tmp/sales_data.json", "w") as f:
        json.dump(data, f)

    agent = Agent.create("agent.acl")
    session = agent.session(".", builtin_skills=True)

    result = session.send("""
    Read /tmp/sales_data.json and create a bar chart
    showing product sales. Use vis-chart format.
    """)

    print(result.text)
    print()

def test_multiple_charts():
    """Test generating multiple charts"""
    print("=" * 60)
    print("Test 3: Multiple Charts")
    print("=" * 60)

    agent = Agent.create("agent.acl")
    session = agent.session(".", builtin_skills=True)

    result = session.send("""
    Create two charts using vis-chart format:

    1. Line chart: Monthly revenue (Jan: 45k, Feb: 52k, Mar: 48k, Apr: 61k)
    2. Pie chart: Browser share (Chrome: 65%, Firefox: 15%, Safari: 12%, Edge: 8%)
    """)

    print(result.text)
    print()

def test_chart_type_selection():
    """Test automatic chart type selection"""
    print("=" * 60)
    print("Test 4: Automatic Chart Type Selection")
    print("=" * 60)

    agent = Agent.create("agent.acl")
    session = agent.session(".", builtin_skills=True)

    result = session.send("""
    I have this data:
    - Speed: 80
    - Reliability: 90
    - Cost: 60
    - Features: 85

    Create the most appropriate visualization using vis-chart format.
    """)

    print(result.text)
    print()

def test_explicit_skill_call():
    """Test explicit skill invocation"""
    print("=" * 60)
    print("Test 5: Explicit Skill Call")
    print("=" * 60)

    agent = Agent.create("agent.acl")
    session = agent.session(".", builtin_skills=True)

    # Register the chart-generator skill if not already registered
    result = session.send("/chart-generator Show sales trend: Q1=100, Q2=120, Q3=150, Q4=180")

    print(result.text)
    print()

if __name__ == "__main__":
    print("\n" + "=" * 60)
    print("Chart Generator Skill Test Suite")
    print("=" * 60 + "\n")

    try:
        test_basic_chart()
        test_data_file_chart()
        test_multiple_charts()
        test_chart_type_selection()
        test_explicit_skill_call()

        print("\n" + "=" * 60)
        print("All tests completed!")
        print("=" * 60 + "\n")

    except Exception as e:
        print(f"\n❌ Error: {e}\n")
        import traceback
        traceback.print_exc()
