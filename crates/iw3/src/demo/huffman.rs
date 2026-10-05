//! The Huffman coding of CoD4 network messages.
//!
//! The code is Quake 3's adaptive Huffman tree, trained once at startup by
//! adding every byte value as many times as a fixed frequency table says,
//! rarest first, and never changed afterwards. Building the same tree means
//! replaying that training exactly: the adaptive algorithm's tie-breaking
//! decides the codes.

const HMAX: usize = 256;
const NYT: u16 = HMAX as u16;
const INTERNAL: u16 = HMAX as u16 + 1;

/// How often each byte value occurs in CoD4 network traffic.
#[rustfmt::skip]
const FREQUENCIES: [u32; HMAX] = [
    274054, 68777, 40460, 40266, 48059, 39006, 48630, 27692, 17712, 15439, 12386, 10758, 9420, 9979, 9346, 15256,
    13184, 14319, 7750, 7221, 6095, 5666, 12606, 7263, 7322, 5807, 11628, 6199, 7826, 6349, 7698, 9656,
    28968, 5164, 13629, 6058, 4745, 4519, 5199, 4807, 5323, 3433, 3455, 3563, 6979, 5229, 5002, 4423,
    14108, 13631, 11908, 11801, 10261, 7635, 7215, 7218, 9353, 6161, 5689, 4649, 5026, 5866, 8002, 10534,
    15381, 8874, 11798, 7199, 12814, 6103, 4982, 5972, 6779, 4929, 5333, 3503, 4345, 6098, 14117, 16440,
    6446, 3062, 4695, 3085, 4198, 4013, 3878, 3414, 5514, 4092, 3261, 4740, 4544, 3127, 3385, 7688,
    11126, 6417, 5297, 4529, 6333, 4210, 7056, 4658, 6190, 3512, 2843, 3479, 9369, 5203, 4980, 5881,
    7509, 4292, 6097, 5492, 4648, 2996, 4988, 4163, 6534, 4001, 4342, 4488, 6039, 4827, 7112, 8654,
    26712, 8688, 9677, 9368, 7209, 3399, 4473, 4677, 11087, 4094, 3404, 4176, 6733, 3702, 11420, 4867,
    5968, 3475, 3722, 3560, 4571, 2720, 3189, 3099, 4595, 4044, 4402, 3889, 4989, 3186, 3153, 5387,
    8020, 3322, 3775, 2886, 4191, 2879, 3110, 2576, 3693, 2436, 4935, 3017, 3538, 5688, 3444, 3410,
    9170, 4708, 3425, 3273, 3684, 4564, 6957, 4817, 5224, 3285, 3143, 4227, 5630, 6053, 5851, 6507,
    13692, 8270, 8260, 5583, 7568, 4082, 3984, 4574, 6440, 3533, 2992, 2708, 5190, 3889, 3799, 4582,
    6020, 3464, 4431, 3495, 2906, 2243, 3856, 3321, 8759, 3928, 2905, 3875, 4382, 3885, 5869, 6235,
    10685, 4433, 4639, 4305, 4683, 2849, 3379, 4684, 5477, 4127, 3853, 3515, 4913, 3601, 5237, 6617,
    9019, 4857, 4112, 5180, 5998, 4925, 4986, 6365, 7930, 5948, 8085, 7732, 8643, 8901, 9653, 32647,
];

#[derive(Clone, Copy, Default)]
struct Node {
    left: Option<u16>,
    right: Option<u16>,
    parent: Option<u16>,
    next: Option<u16>,
    prev: Option<u16>,
    /// Slot holding the leader of this node's weight block.
    head: Option<u16>,
    weight: u32,
    symbol: u16,
}

/// The adaptive tree while it is being trained.
struct Builder {
    nodes: Vec<Node>,
    tree: u16,
    lhead: u16,
    loc: [Option<u16>; HMAX + 1],
    /// Block-leader slots, and the free ones (reused last-freed first).
    slots: Vec<Option<u16>>,
    free: Vec<u16>,
}

impl Builder {
    fn new() -> Builder {
        let nyt = Node { symbol: NYT, ..Default::default() };
        let mut loc = [None; HMAX + 1];
        loc[NYT as usize] = Some(0);
        Builder { nodes: vec![nyt], tree: 0, lhead: 0, loc, slots: Vec::new(), free: Vec::new() }
    }

    fn n(&mut self, i: u16) -> &mut Node {
        &mut self.nodes[i as usize]
    }

    fn new_slot(&mut self) -> u16 {
        match self.free.pop() {
            Some(s) => s,
            None => {
                self.slots.push(None);
                (self.slots.len() - 1) as u16
            }
        }
    }

    fn free_slot(&mut self, s: u16) {
        self.slots[s as usize] = None;
        self.free.push(s);
    }

    /// Swap two nodes' places in the tree.
    fn swap(&mut self, a: u16, b: u16) {
        let (pa, pb) = (self.n(a).parent, self.n(b).parent);
        match pa {
            Some(p) if self.n(p).left == Some(a) => self.n(p).left = Some(b),
            Some(p) => self.n(p).right = Some(b),
            None => self.tree = b,
        }
        match pb {
            Some(p) if self.n(p).left == Some(b) => self.n(p).left = Some(a),
            Some(p) => self.n(p).right = Some(a),
            None => self.tree = a,
        }
        self.n(a).parent = pb;
        self.n(b).parent = pa;
    }

    /// Swap two nodes' places in the weight-ordered list.
    fn swap_list(&mut self, a: u16, b: u16) {
        let t = self.n(a).next;
        self.n(a).next = self.n(b).next;
        self.n(b).next = t;
        let t = self.n(a).prev;
        self.n(a).prev = self.n(b).prev;
        self.n(b).prev = t;
        if self.n(a).next == Some(a) {
            self.n(a).next = Some(b);
        }
        if self.n(b).next == Some(b) {
            self.n(b).next = Some(a);
        }
        for x in [a, b] {
            if let Some(nx) = self.n(x).next {
                self.n(nx).prev = Some(x);
            }
        }
        for x in [a, b] {
            if let Some(pv) = self.n(x).prev {
                self.n(pv).next = Some(x);
            }
        }
    }

    fn increment(&mut self, node: Option<u16>) {
        let Some(node) = node else { return };
        let weight = self.n(node).weight;
        if self.n(node).next.is_some_and(|nx| self.nodes[nx as usize].weight == weight) {
            let head = self.n(node).head.expect("head");
            let leader = self.slots[head as usize].expect("leader");
            if Some(leader) != self.n(node).parent {
                self.swap(leader, node);
            }
            self.swap_list(leader, node);
        }
        let head = self.n(node).head.expect("head");
        let prev = self.n(node).prev;
        match prev {
            Some(pv) if self.nodes[pv as usize].weight == weight => self.slots[head as usize] = Some(pv),
            _ => self.free_slot(head),
        }
        self.n(node).weight += 1;
        let weight = weight + 1;
        let next = self.n(node).next;
        match next {
            Some(nx) if self.nodes[nx as usize].weight == weight => {
                let h = self.nodes[nx as usize].head;
                self.n(node).head = h;
            }
            _ => {
                let s = self.new_slot();
                self.slots[s as usize] = Some(node);
                self.n(node).head = Some(s);
            }
        }
        if let Some(parent) = self.n(node).parent {
            self.increment(Some(parent));
            if self.n(node).prev == Some(parent) {
                self.swap_list(node, parent);
                let head = self.n(node).head.expect("head");
                if self.slots[head as usize] == Some(node) {
                    self.slots[head as usize] = Some(parent);
                }
            }
        }
    }

    fn add(&mut self, symbol: u8) {
        if let Some(leaf) = self.loc[symbol as usize] {
            return self.increment(Some(leaf));
        }
        // First time: split the NYT node into an internal node with the NYT
        // on the left and the new symbol on the right.
        let lhead = self.lhead;
        let leaf = self.nodes.len() as u16;
        let internal = leaf + 1;
        self.nodes.push(Node { symbol: symbol as u16, weight: 1, ..Default::default() });
        self.nodes.push(Node { symbol: INTERNAL, weight: 1, ..Default::default() });
        for new in [internal, leaf] {
            let next = self.n(lhead).next;
            self.n(new).next = next;
            let head = match next {
                Some(nx) => {
                    self.n(nx).prev = Some(new);
                    if self.nodes[nx as usize].weight == 1 { self.nodes[nx as usize].head } else { None }
                }
                None => None,
            };
            let head = head.unwrap_or_else(|| {
                let s = self.new_slot();
                self.slots[s as usize] = Some(new);
                s
            });
            self.n(new).head = Some(head);
            self.n(lhead).next = Some(new);
            self.n(new).prev = Some(lhead);
        }
        let parent = self.n(lhead).parent;
        match parent {
            Some(p) if self.n(p).left == Some(lhead) => self.n(p).left = Some(internal),
            Some(p) => self.n(p).right = Some(internal),
            None => self.tree = internal,
        }
        let n = self.n(internal);
        n.right = Some(leaf);
        n.left = Some(lhead);
        n.parent = parent;
        self.n(lhead).parent = Some(internal);
        self.n(leaf).parent = Some(internal);
        self.loc[symbol as usize] = Some(leaf);
        self.increment(parent);
    }
}

/// The trained decoding tree.
pub struct Huffman {
    /// Per node: (left, right) children for internal nodes, else the symbol.
    nodes: Vec<Result<(u16, u16), u16>>,
    root: u16,
}

impl Huffman {
    pub fn cod4() -> Huffman {
        let mut b = Builder::new();
        let mut done = [false; HMAX];
        // Rarest first; ties go to the lower byte value.
        while let Some(i) = (0..HMAX).filter(|&i| !done[i]).min_by_key(|&i| (FREQUENCIES[i], i)) {
            for _ in 0..FREQUENCIES[i] {
                b.add(i as u8);
            }
            done[i] = true;
        }
        let nodes = b
            .nodes
            .iter()
            .map(|n| if n.symbol == INTERNAL { Ok((n.left.unwrap_or(0), n.right.unwrap_or(0))) } else { Err(n.symbol) })
            .collect();
        Huffman { nodes, root: b.tree }
    }

    /// Decode `input` (LSB-first bits) until it runs out.
    pub fn decompress(&self, input: &[u8]) -> Vec<u8> {
        let total = input.len() * 8;
        let mut out = Vec::with_capacity(input.len() * 2);
        let mut bit = 0;
        'symbols: while bit < total {
            let mut node = self.root;
            loop {
                match self.nodes[node as usize] {
                    Ok((left, right)) => {
                        if bit >= total {
                            break 'symbols;
                        }
                        let b = (input[bit >> 3] >> (bit & 7)) & 1;
                        bit += 1;
                        node = if b == 1 { right } else { left };
                    }
                    Err(symbol) => {
                        out.push(symbol as u8);
                        break;
                    }
                }
            }
        }
        out
    }
}
