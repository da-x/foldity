# foldity

Utility for folding terminal output to fit the screen.

- Presented as a tree.
- Based on matching lines that mark 'start' and 'end'.
- Leafs finishing executing are minimized.

## Example usage

Let's say we have a command outputting a lot of text plus `>> [start]` and `<< <return-code>` marker pairs:

```
<some command with lots of output> | foldity -s '>>( (?P<M>.*))?' -e '<<( (?P<M>.*))?'
```

The screen capture below was generated with the input from [test/data.txt](test/data.txt) being fed slowly into foldity.

<img src="https://user-images.githubusercontent.com/321273/98439699-61d30980-20fc-11eb-9e6f-5615ed8e63d8.gif">
